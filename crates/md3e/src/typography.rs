//! The M3 type scale on Roboto Flex.
//!
//! One variable font file is registered once per weight we use, each as its
//! own `FontFamily::Name("roboto-flex-<weight>")` with the `wght` axis pinned
//! through `FontData::tweak.coords`. egui has no per-text weight selection
//! except through such families (or `TextFormat::coords`).
//!
//! Tabular figures: Roboto Flex's default digits are already tabular (all ten
//! share one advance; the font's only figure feature is the opt-in `pnum`),
//! so time labels do not jitter. egui cannot select OpenType features anyway.
//!
//! Only `wght` is varied. `opsz` stays at the font's default (14), so large
//! display text is not optically adjusted.

use std::sync::Arc;

use egui::{
    Align, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText, TextFormat,
    WidgetText,
    epaint::text::VariationCoords,
    text::LayoutJob,
};

/// Roboto Flex variable font (SIL OFL, see `assets/fonts/OFL.txt`).
static ROBOTO_FLEX: &[u8] = include_bytes!("../../../assets/fonts/RobotoFlex-VF.ttf");

/// The weights registered as families.
pub const WEIGHTS: [u16; 4] = [400, 500, 600, 700];

fn font_key(weight: u16) -> String {
    format!("roboto-flex-{weight}")
}

/// The registered weight closest to `weight`.
pub fn snap_weight(weight: u16) -> u16 {
    *WEIGHTS
        .iter()
        .min_by_key(|w| w.abs_diff(weight))
        .unwrap_or(&400)
}

/// The font family for `weight` (snapped to 400/500/600/700). It exists once
/// [`install_fonts`] has run.
pub fn family(weight: u16) -> FontFamily {
    FontFamily::Name(font_key(snap_weight(weight)).into())
}

/// Registers Roboto Flex: one family per weight, each falling back to the
/// fonts egui already had for `Proportional` (symbols, emoji, CJK...), and
/// the 400 face as the first proportional font.
pub fn install_fonts(defs: &mut FontDefinitions) {
    let fallbacks = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for weight in WEIGHTS {
        let key = font_key(weight);
        let mut data = FontData::from_static(ROBOTO_FLEX);
        data.tweak.coords = VariationCoords::new([(b"wght", f32::from(weight))]);
        defs.font_data.insert(key.clone(), Arc::new(data));

        let mut stack = vec![key.clone()];
        stack.extend(
            fallbacks
                .iter()
                .filter(|f| !f.starts_with("roboto-flex-"))
                .cloned(),
        );
        defs.families
            .insert(FontFamily::Name(key.clone().into()), stack);
    }
    let proportional = defs.families.entry(FontFamily::Proportional).or_default();
    proportional.retain(|f| !f.starts_with("roboto-flex-"));
    proportional.insert(0, font_key(400));
}

/// One entry of the type scale: size, line height and tracking in points
/// (1 dp = 1 point), weight on the 100..=900 scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    pub size: f32,
    pub line_height: f32,
    pub weight: u16,
    pub tracking: f32,
}

impl TypeStyle {
    pub const fn new(size: f32, line_height: f32, weight: u16, tracking: f32) -> Self {
        Self {
            size,
            line_height,
            weight,
            tracking,
        }
    }

    pub fn with_weight(self, weight: u16) -> Self {
        Self { weight, ..self }
    }

    pub fn font_id(&self) -> FontId {
        FontId::new(self.size, family(self.weight))
    }

    /// For `LayoutJob`s: font, tracking as letter spacing, line height.
    pub fn text_format(&self, color: Color32) -> TextFormat {
        TextFormat {
            font_id: self.font_id(),
            extra_letter_spacing: self.tracking,
            line_height: Some(self.line_height),
            color,
            ..TextFormat::default()
        }
    }

    /// A one-line job with the text centred in its line box.
    ///
    /// egui puts glyphs at the top of a row and all extra line height below
    /// them, so a `line_height` line centred by its galley sits 2-3 px high.
    /// Here the text keeps its natural height inside a row forced to
    /// `line_height` (`first_row_min_height`) with centre valign, which
    /// splits the extra evenly. Only the first row gets it: wrapping
    /// paragraphs use [`Self::text_format`] / [`Self::rich_para`].
    pub fn job(&self, text: impl Into<String>, color: Color32) -> LayoutJob {
        let mut job = LayoutJob::single_section(
            text.into(),
            TextFormat {
                line_height: None,
                valign: Align::Center,
                ..self.text_format(color)
            },
        );
        job.first_row_min_height = self.line_height;
        job
    }

    /// For one-line `Label`s and buttons (see [`Self::job`]).
    pub fn rich(&self, text: impl Into<String>, color: Color32) -> WidgetText {
        WidgetText::from(self.job(text, color))
    }

    /// For `Label`s that wrap into paragraphs: every row `line_height` tall.
    pub fn rich_para(&self, text: impl Into<String>, color: Color32) -> RichText {
        RichText::new(text)
            .font(self.font_id())
            .extra_letter_spacing(self.tracking)
            .line_height(Some(self.line_height))
            .color(color)
    }
}

/// The 15 baseline styles (tokens.md section 2.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TypeRole {
    DisplayLarge,
    DisplayMedium,
    DisplaySmall,
    HeadlineLarge,
    HeadlineMedium,
    HeadlineSmall,
    TitleLarge,
    TitleMedium,
    TitleSmall,
    BodyLarge,
    BodyMedium,
    BodySmall,
    LabelLarge,
    LabelMedium,
    LabelSmall,
}

impl TypeRole {
    pub const ALL: [Self; 15] = [
        Self::DisplayLarge,
        Self::DisplayMedium,
        Self::DisplaySmall,
        Self::HeadlineLarge,
        Self::HeadlineMedium,
        Self::HeadlineSmall,
        Self::TitleLarge,
        Self::TitleMedium,
        Self::TitleSmall,
        Self::BodyLarge,
        Self::BodyMedium,
        Self::BodySmall,
        Self::LabelLarge,
        Self::LabelMedium,
        Self::LabelSmall,
    ];

    /// The baseline style.
    pub const fn style(self) -> TypeStyle {
        match self {
            Self::DisplayLarge => TypeStyle::new(57.0, 64.0, 400, -0.25),
            Self::DisplayMedium => TypeStyle::new(45.0, 52.0, 400, 0.0),
            Self::DisplaySmall => TypeStyle::new(36.0, 44.0, 400, 0.0),
            Self::HeadlineLarge => TypeStyle::new(32.0, 40.0, 400, 0.0),
            Self::HeadlineMedium => TypeStyle::new(28.0, 36.0, 400, 0.0),
            Self::HeadlineSmall => TypeStyle::new(24.0, 32.0, 400, 0.0),
            Self::TitleLarge => TypeStyle::new(22.0, 28.0, 400, 0.0),
            Self::TitleMedium => TypeStyle::new(16.0, 24.0, 500, 0.15),
            Self::TitleSmall => TypeStyle::new(14.0, 20.0, 500, 0.1),
            Self::BodyLarge => TypeStyle::new(16.0, 24.0, 400, 0.5),
            Self::BodyMedium => TypeStyle::new(14.0, 20.0, 400, 0.25),
            Self::BodySmall => TypeStyle::new(12.0, 16.0, 400, 0.4),
            Self::LabelLarge => TypeStyle::new(14.0, 20.0, 500, 0.1),
            Self::LabelMedium => TypeStyle::new(12.0, 16.0, 500, 0.5),
            Self::LabelSmall => TypeStyle::new(11.0, 16.0, 500, 0.5),
        }
    }

    /// The Expressive emphasized style: same metrics, heavier weight. 500 for
    /// display, headline, title-large and body; 600 for title-medium/small
    /// and labels (we have a variable font, so 600 rather than Compose's 700).
    pub const fn emphasized(self) -> TypeStyle {
        let base = self.style();
        let weight = match self {
            Self::DisplayLarge
            | Self::DisplayMedium
            | Self::DisplaySmall
            | Self::HeadlineLarge
            | Self::HeadlineMedium
            | Self::HeadlineSmall
            | Self::TitleLarge
            | Self::BodyLarge
            | Self::BodyMedium
            | Self::BodySmall => 500,
            Self::TitleMedium
            | Self::TitleSmall
            | Self::LabelLarge
            | Self::LabelMedium
            | Self::LabelSmall => 600,
        };
        TypeStyle { weight, ..base }
    }

    pub fn font_id(self) -> FontId {
        self.style().font_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_matches_tokens_md() {
        let s = TypeRole::DisplayLarge.style();
        assert_eq!(
            (s.size, s.line_height, s.weight, s.tracking),
            (57.0, 64.0, 400, -0.25)
        );
        let s = TypeRole::TitleMedium.style();
        assert_eq!(
            (s.size, s.line_height, s.weight, s.tracking),
            (16.0, 24.0, 500, 0.15)
        );
        let s = TypeRole::LabelSmall.style();
        assert_eq!(
            (s.size, s.line_height, s.weight, s.tracking),
            (11.0, 16.0, 500, 0.5)
        );
        assert_eq!(TypeRole::ALL.len(), 15);
    }

    #[test]
    fn emphasized_changes_only_weight() {
        for role in TypeRole::ALL {
            let (b, e) = (role.style(), role.emphasized());
            assert_eq!(
                (b.size, b.line_height, b.tracking),
                (e.size, e.line_height, e.tracking)
            );
            assert!(e.weight > b.weight, "{role:?}");
        }
        assert_eq!(TypeRole::BodyLarge.emphasized().weight, 500);
        assert_eq!(TypeRole::LabelLarge.emphasized().weight, 600);
    }

    #[test]
    fn weights_snap_to_registered_faces() {
        assert_eq!(snap_weight(300), 400);
        assert_eq!(snap_weight(450), 400);
        assert_eq!(snap_weight(550), 500);
        assert_eq!(snap_weight(700), 700);
        assert_eq!(snap_weight(900), 700);
    }

    #[test]
    fn install_registers_every_weight_with_fallbacks() {
        let mut defs = FontDefinitions::default();
        let before = defs.families[&FontFamily::Proportional].clone();
        install_fonts(&mut defs);
        install_fonts(&mut defs); // idempotent
        for weight in WEIGHTS {
            let stack = &defs.families[&family(weight)];
            assert_eq!(stack[0], font_key(weight));
            assert_eq!(stack.len(), 1 + before.len());
            let data = &defs.font_data[&font_key(weight)];
            assert_eq!(
                data.tweak.coords,
                VariationCoords::new([(b"wght", f32::from(weight))])
            );
        }
        let prop = &defs.families[&FontFamily::Proportional];
        assert_eq!(prop[0], "roboto-flex-400");
        assert_eq!(prop.len(), 1 + before.len());
    }

    #[test]
    fn text_format_carries_tracking_and_line_height() {
        let f = TypeRole::BodyLarge.style().text_format(Color32::WHITE);
        assert_eq!(f.extra_letter_spacing, 0.5);
        assert_eq!(f.line_height, Some(24.0));
        assert_eq!(f.font_id.size, 16.0);
    }

    #[test]
    fn text_lays_out_with_the_registered_fonts() {
        let ctx = egui::Context::default();
        let mut defs = FontDefinitions::default();
        install_fonts(&mut defs);
        ctx.set_fonts(defs);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let thin = ui.painter().layout_no_wrap(
                "Привет 0:42".into(),
                TypeRole::BodyLarge.font_id(),
                Color32::WHITE,
            );
            let bold = ui.painter().layout_no_wrap(
                "Привет 0:42".into(),
                TypeRole::BodyLarge.emphasized().with_weight(700).font_id(),
                Color32::WHITE,
            );
            assert!(thin.size().x > 10.0);
            assert!(bold.size().x > thin.size().x);
        });
        out.textures_delta.clear();
    }
}
