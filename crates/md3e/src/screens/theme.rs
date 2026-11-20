//! Colour scheme animation and the scheme helpers of the screens.

use std::collections::HashMap;

use nikgob_motion::Spring;
use nikgob_view::ThemeMode;

use super::*;
use crate::{ColorScheme, Contrast, SchemeVariant};

pub(super) type SchemeKey = (u32, bool, SchemeVariant, Contrast);

/// «Моя волна» (or a card started as one) is what the queue plays.
pub(crate) fn wave_plays(state: &nikgob_view::ViewState) -> bool {
    state.queue.radio.as_ref().is_some_and(|r| r.wave.is_some())
}

/// Where [`Md3eUi`] keeps the seed the scheme is made from.
pub(crate) fn scheme_seed_id() -> egui::Id {
    egui::Id::new("md3e-scheme-seed")
}

/// Dark or light, the scheme variant and the contrast `prefs` ask for.
pub(crate) fn theme_key(ctx: &Context, prefs: &nikgob_view::Prefs) -> (bool, SchemeVariant, Contrast) {
    let dark = match prefs.theme {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => ctx.system_theme() != Some(egui::Theme::Light),
    };
    let variant = match prefs.scheme_variant {
        1 => SchemeVariant::TonalSpot,
        2 => SchemeVariant::Vibrant,
        3 => SchemeVariant::Expressive,
        4 => SchemeVariant::Fidelity,
        5 => SchemeVariant::Monochrome,
        6 => SchemeVariant::Neutral,
        _ => SchemeVariant::Content,
    };
    let contrast = match prefs.contrast {
        1 => Contrast::Medium,
        2 => Contrast::High,
        _ => Contrast::Standard,
    };
    (dark, variant, contrast)
}

/// Animates the whole scheme from one palette to the next (slow effects).
pub(super) struct SchemeAnim {
    key: Option<SchemeKey>,
    from: ColorScheme,
    to: ColorScheme,
    k: Animated<f32>,
    cache: HashMap<SchemeKey, ColorScheme>,
}

impl Default for SchemeAnim {
    fn default() -> Self {
        let base = ColorScheme::baseline(true);
        Self {
            key: None,
            from: base,
            to: base,
            k: Animated::new(1.0, crate::tokens::Motion::expressive().slow_effects)
                .with_threshold(0.001),
            cache: HashMap::new(),
        }
    }
}

impl SchemeAnim {
    pub(super) fn frame(&mut self, ctx: &Context, key: SchemeKey, spring: Spring) -> ColorScheme {
        let t = anim::now(ctx);
        if self.key != Some(key) {
            let current = ColorScheme::lerp(&self.from, &self.to, self.k.value(t));
            let to = *self
                .cache
                .entry(key)
                .or_insert_with(|| ColorScheme::from_seed(key.0, key.2, key.1, key.3));
            if self.key.is_none() {
                self.from = to;
                self.k.snap(1.0);
            } else {
                self.from = current;
                self.k.snap(0.0);
                self.k.animate_to(1.0, spring, t);
            }
            self.to = to;
            self.key = Some(key);
        }
        if !self.k.is_settled(t) {
            ctx.request_repaint();
        }
        ColorScheme::lerp(&self.from, &self.to, self.k.value(t).clamp(0.0, 1.0))
    }
}

/// The spring Now Playing opens and closes on (`Prefs::np_motion`):
/// smooth, lively (a quick one with a little bounce) or fast.
pub(crate) fn np_spring(motion: u8) -> nikgob_motion::Spring {
    match motion {
        0 => nikgob_motion::Spring::new(0.85, 220.0),
        2 => nikgob_motion::Spring::new(0.9, 1000.0),
        _ => nikgob_motion::Spring::new(0.72, 520.0),
    }
}

/// Pages mini mode shows in place of Now Playing: settings, and the artist
/// opened from the player, «Карта сети».
pub(super) fn compact_page(route: &Route) -> bool {
    matches!(route, Route::Settings | Route::Artist(_) | Route::Devices)
}
