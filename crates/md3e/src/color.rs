//! Material 3 colour roles generated from a seed colour.
//!
//! [`ColorScheme::from_seed`] runs `material-colors`' dynamic-colour schemes
//! (the port of `material-color-utilities`) and keeps the roles an egui UI
//! needs. [`seed_from_image`] picks the seed from album art as Android does:
//! quantize, then score.

use egui::Color32;
use material_colors::{
    color::Argb,
    dynamic_color::{DynamicScheme, Variant},
    quantize::{Quantizer, QuantizerCelebi},
    scheme::Scheme,
    score::Score,
};

/// The M3 baseline seed (`#6750A4`).
pub const BASELINE_SEED: u32 = 0xFF67_50A4;

/// Which `material-color-utilities` scheme turns the seed into palettes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum SchemeVariant {
    /// Stays faithful to the source colour; our default for album art.
    #[default]
    Content,
    /// Android's default: muted, medium chroma.
    TonalSpot,
    Vibrant,
    Expressive,
    /// Like Content, but the container tones follow the source exactly.
    Fidelity,
    Monochrome,
    Neutral,
}

impl SchemeVariant {
    pub const ALL: [Self; 7] = [
        Self::Content,
        Self::TonalSpot,
        Self::Vibrant,
        Self::Expressive,
        Self::Fidelity,
        Self::Monochrome,
        Self::Neutral,
    ];

    fn to_material(self) -> Variant {
        match self {
            Self::Content => Variant::Content,
            Self::TonalSpot => Variant::TonalSpot,
            Self::Vibrant => Variant::Vibrant,
            Self::Expressive => Variant::Expressive,
            Self::Fidelity => Variant::Fidelity,
            Self::Monochrome => Variant::Monochrome,
            Self::Neutral => Variant::Neutral,
        }
    }
}

/// The user's contrast preference.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Contrast {
    #[default]
    Standard,
    /// At least 3:1 on the relevant pairs.
    Medium,
    /// At least 7:1.
    High,
}

impl Contrast {
    /// The contrast level `material-color-utilities` takes (-1..=1).
    pub const fn level(self) -> f64 {
        match self {
            Self::Standard => 0.0,
            Self::Medium => 0.5,
            Self::High => 1.0,
        }
    }
}

fn color(argb: Argb) -> Color32 {
    Color32::from_rgb(argb.red, argb.green, argb.blue)
}

/// Declares [`ColorScheme`] once: the field list drives the struct, the
/// conversion from `material-colors`, interpolation and iteration.
macro_rules! color_scheme {
    ($($(#[$doc:meta])* $field:ident),* $(,)?) => {
        /// Every M3 colour role as an opaque egui colour.
        ///
        /// The deprecated `surface-tint`, `surface-variant` and `background`
        /// roles are left out; use `surface-container-*` and `surface`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct ColorScheme {
            $($(#[$doc])* pub $field: Color32,)*
        }

        impl ColorScheme {
            /// Every role's name and colour, in declaration order.
            pub fn roles(&self) -> Vec<(&'static str, Color32)> {
                vec![$((stringify!($field), self.$field),)*]
            }

            fn from_material(scheme: &Scheme) -> Self {
                Self { $($field: color(scheme.$field),)* }
            }

            /// Each role moved `t` of the way from `a` to `b`.
            ///
            /// `t` may leave 0..=1 (a spring overshoots); channels clamp.
            pub fn lerp(a: &Self, b: &Self, t: f32) -> Self {
                Self { $($field: lerp_color(a.$field, b.$field, t),)* }
            }
        }
    };
}

color_scheme! {
    primary, on_primary, primary_container, on_primary_container,
    secondary, on_secondary, secondary_container, on_secondary_container,
    tertiary, on_tertiary, tertiary_container, on_tertiary_container,
    error, on_error, error_container, on_error_container,
    surface, surface_dim, surface_bright,
    surface_container_lowest, surface_container_low, surface_container,
    surface_container_high, surface_container_highest,
    on_surface, on_surface_variant,
    outline, outline_variant,
    inverse_surface, inverse_on_surface, inverse_primary,
    primary_fixed, primary_fixed_dim, on_primary_fixed, on_primary_fixed_variant,
    secondary_fixed, secondary_fixed_dim, on_secondary_fixed, on_secondary_fixed_variant,
    tertiary_fixed, tertiary_fixed_dim, on_tertiary_fixed, on_tertiary_fixed_variant,
    /// Drawn at [`SCRIM_OPACITY`]; see [`ColorScheme::scrim_overlay`].
    scrim,
    shadow,
}

/// Opacity of the scrim behind modal surfaces.
pub const SCRIM_OPACITY: f32 = 0.32;

impl ColorScheme {
    /// A scheme from an `0xAARRGGBB` (alpha ignored) seed.
    pub fn from_seed(argb: u32, variant: SchemeVariant, dark: bool, contrast: Contrast) -> Self {
        let source = Argb::from_u32(argb | 0xFF00_0000);
        let scheme =
            DynamicScheme::by_variant(source, &variant.to_material(), dark, Some(contrast.level()));
        Self::from_material(&Scheme::from(scheme))
    }

    /// The static M3 baseline scheme (purple), exactly as tokens.md section
    /// 1.2 lists it. This is the fallback when there is no seed.
    ///
    /// It is not `from_seed(BASELINE_SEED, TonalSpot, ..)`: the published
    /// baseline comes from the legacy core palette (primary chroma 48, error
    /// key `#B3261E`), while TonalSpot uses primary chroma 36, so e.g. its
    /// light primary is `#65558F`.
    pub const fn baseline(dark: bool) -> Self {
        if dark { BASELINE_DARK } else { BASELINE_LIGHT }
    }

    /// The scrim colour at its standard 32% opacity.
    pub fn scrim_overlay(&self) -> Color32 {
        self.scrim.gamma_multiply(SCRIM_OPACITY)
    }
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::baseline(true)
    }
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// tokens.md section 1.2, light column.
const BASELINE_LIGHT: ColorScheme = ColorScheme {
    primary: hex(0x6750A4),
    on_primary: hex(0xFFFFFF),
    primary_container: hex(0xEADDFF),
    on_primary_container: hex(0x21005D),
    secondary: hex(0x625B71),
    on_secondary: hex(0xFFFFFF),
    secondary_container: hex(0xE8DEF8),
    on_secondary_container: hex(0x1D192B),
    tertiary: hex(0x7D5260),
    on_tertiary: hex(0xFFFFFF),
    tertiary_container: hex(0xFFD8E4),
    on_tertiary_container: hex(0x31111D),
    error: hex(0xB3261E),
    on_error: hex(0xFFFFFF),
    error_container: hex(0xF9DEDC),
    on_error_container: hex(0x410E0B),
    surface: hex(0xFEF7FF),
    surface_dim: hex(0xDED8E1),
    surface_bright: hex(0xFEF7FF),
    surface_container_lowest: hex(0xFFFFFF),
    surface_container_low: hex(0xF7F2FA),
    surface_container: hex(0xF3EDF7),
    surface_container_high: hex(0xECE6F0),
    surface_container_highest: hex(0xE6E0E9),
    on_surface: hex(0x1D1B20),
    on_surface_variant: hex(0x49454F),
    outline: hex(0x79747E),
    outline_variant: hex(0xCAC4D0),
    inverse_surface: hex(0x322F35),
    inverse_on_surface: hex(0xF5EFF7),
    inverse_primary: hex(0xD0BCFF),
    primary_fixed: hex(0xEADDFF),
    primary_fixed_dim: hex(0xD0BCFF),
    on_primary_fixed: hex(0x21005D),
    on_primary_fixed_variant: hex(0x4F378B),
    secondary_fixed: hex(0xE8DEF8),
    secondary_fixed_dim: hex(0xCCC2DC),
    on_secondary_fixed: hex(0x1D192B),
    on_secondary_fixed_variant: hex(0x4A4458),
    tertiary_fixed: hex(0xFFD8E4),
    tertiary_fixed_dim: hex(0xEFB8C8),
    on_tertiary_fixed: hex(0x31111D),
    on_tertiary_fixed_variant: hex(0x633B48),
    scrim: hex(0x000000),
    shadow: hex(0x000000),
};

/// tokens.md section 1.2, dark column.
const BASELINE_DARK: ColorScheme = ColorScheme {
    primary: hex(0xD0BCFF),
    on_primary: hex(0x381E72),
    primary_container: hex(0x4F378B),
    on_primary_container: hex(0xEADDFF),
    secondary: hex(0xCCC2DC),
    on_secondary: hex(0x332D41),
    secondary_container: hex(0x4A4458),
    on_secondary_container: hex(0xE8DEF8),
    tertiary: hex(0xEFB8C8),
    on_tertiary: hex(0x492532),
    tertiary_container: hex(0x633B48),
    on_tertiary_container: hex(0xFFD8E4),
    error: hex(0xF2B8B5),
    on_error: hex(0x601410),
    error_container: hex(0x8C1D18),
    on_error_container: hex(0xF9DEDC),
    surface: hex(0x141218),
    surface_dim: hex(0x141218),
    surface_bright: hex(0x3B383E),
    surface_container_lowest: hex(0x0F0D13),
    surface_container_low: hex(0x1D1B20),
    surface_container: hex(0x211F26),
    surface_container_high: hex(0x2B2930),
    surface_container_highest: hex(0x36343B),
    on_surface: hex(0xE6E0E9),
    on_surface_variant: hex(0xCAC4D0),
    outline: hex(0x938F99),
    outline_variant: hex(0x49454F),
    inverse_surface: hex(0xE6E0E9),
    inverse_on_surface: hex(0x322F35),
    inverse_primary: hex(0x6750A4),
    primary_fixed: hex(0xEADDFF),
    primary_fixed_dim: hex(0xD0BCFF),
    on_primary_fixed: hex(0x21005D),
    on_primary_fixed_variant: hex(0x4F378B),
    secondary_fixed: hex(0xE8DEF8),
    secondary_fixed_dim: hex(0xCCC2DC),
    on_secondary_fixed: hex(0x1D192B),
    on_secondary_fixed_variant: hex(0x4A4458),
    tertiary_fixed: hex(0xFFD8E4),
    tertiary_fixed_dim: hex(0xEFB8C8),
    on_tertiary_fixed: hex(0x31111D),
    on_tertiary_fixed_variant: hex(0x633B48),
    scrim: hex(0x000000),
    shadow: hex(0x000000),
};

/// `a` moved `t` of the way to `b`, per channel in sRGB space. The channels
/// are premultiplied, so translucent colours blend correctly too.
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let ch = |a: u8, b: u8| {
        let (a, b) = (f32::from(a), f32::from(b));
        (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_premultiplied(
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
        ch(a.a(), b.a()),
    )
}

/// `c` at `opacity` (0..=1) of its own alpha; for state layers and disabled
/// content.
pub fn with_opacity(c: Color32, opacity: f32) -> Color32 {
    c.gamma_multiply(opacity)
}

/// `overlay` painted over the opaque `base` at `opacity`: what a state layer
/// looks like on its container.
pub fn over(base: Color32, overlay: Color32, opacity: f32) -> Color32 {
    lerp_color(base, overlay, opacity)
}

/// WCAG 2 contrast ratio between two colours (1.0..=21.0); alpha is ignored.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Longest side the image is sampled down to before quantizing.
const SEED_SAMPLE_SIDE: usize = 128;
/// Colours the quantizer clusters to, as Android's wallpaper colours do.
const SEED_QUANTIZE_COLORS: usize = 128;

/// Picks a seed colour (`0xFFRRGGBB`) from `rgba` (straight alpha, row-major,
/// `width * height * 4` bytes).
///
/// The image is sampled down to at most 128 px, quantized with Celebi (Wu +
/// weighted k-means) and ranked by [`Score`], which drops colours with
/// chroma below 5 or under 1% of the image. Returns `None` for greyscale or
/// otherwise colourless art, and for empty or mismatched input, so the caller
/// can fall back to the seed from settings.
pub fn seed_from_image(rgba: &[u8], width: usize, height: usize) -> Option<u32> {
    if width == 0 || height == 0 || rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let scale = (width.max(height).div_ceil(SEED_SAMPLE_SIDE)).max(1);
    let (out_w, out_h) = (width.div_ceil(scale), height.div_ceil(scale));
    let mut pixels = Vec::with_capacity(out_w * out_h);
    for oy in 0..out_h {
        // Nearest sample from the middle of each block keeps real colours.
        let y = (oy * scale + scale / 2).min(height - 1);
        for ox in 0..out_w {
            let x = (ox * scale + scale / 2).min(width - 1);
            let i = (y * width + x) * 4;
            if rgba[i + 3] >= 128 {
                pixels.push(Argb::new(255, rgba[i], rgba[i + 1], rgba[i + 2]));
            }
        }
    }
    if pixels.is_empty() {
        return None;
    }
    let result = QuantizerCelebi::quantize(&pixels, SEED_QUANTIZE_COLORS);
    // A transparent fallback marks "nothing suitable".
    let none = Argb::new(0, 0, 0, 0);
    let ranked = Score::score(&result.color_to_count, Some(1), Some(none), Some(true));
    let best = *ranked.first()?;
    (best.alpha != 0).then(|| u32::from_be_bytes([0xFF, best.red, best.green, best.blue]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_str(c: Color32) -> String {
        format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
    }

    fn near(a: Color32, b: Color32) -> bool {
        a.r().abs_diff(b.r()) <= 1 && a.g().abs_diff(b.g()) <= 1 && a.b().abs_diff(b.b()) <= 1
    }

    /// Roles where TonalSpot (chroma 36 primary, neutral chroma 6) departs
    /// from the legacy baseline palette in tokens.md; see `baseline`.
    const TONAL_SPOT_DEVIATIONS: &[&str] = &[
        "primary",
        "on_primary",
        "primary_container",
        "on_primary_container",
        "error",
        "on_error",
        "error_container",
        "on_error_container",
        "surface_container",
        "surface_container_high",
        "inverse_primary",
        "on_primary_fixed",
        "on_primary_fixed_variant",
    ];

    #[test]
    fn tonal_spot_matches_baseline_except_known_deviations() {
        let mut misses = vec![];
        for dark in [false, true] {
            let got = ColorScheme::from_seed(
                BASELINE_SEED,
                SchemeVariant::TonalSpot,
                dark,
                Contrast::Standard,
            );
            let want = ColorScheme::baseline(dark);
            for ((name, g), (_, w)) in got.roles().into_iter().zip(want.roles()) {
                if !near(g, w) && !TONAL_SPOT_DEVIATIONS.contains(&name) {
                    misses.push(format!(
                        "dark={dark} {name}: got {}, tokens.md {}",
                        hex_str(g),
                        hex_str(w)
                    ));
                }
            }
        }
        assert!(
            misses.is_empty(),
            "unexpected mismatches:\n{}",
            misses.join("\n")
        );
    }

    #[test]
    fn legacy_core_palette_reproduces_baseline_accents() {
        use material_colors::scheme::SchemeFromPalette;
        let src = Argb::from_u32(BASELINE_SEED);
        for dark in [false, true] {
            let legacy = if dark {
                SchemeFromPalette::dark(src)
            } else {
                SchemeFromPalette::light(src)
            };
            let want = ColorScheme::baseline(dark);
            let pairs = [
                ("primary", legacy.primary, want.primary),
                ("on_primary", legacy.on_primary, want.on_primary),
                (
                    "primary_container",
                    legacy.primary_container,
                    want.primary_container,
                ),
                (
                    "on_primary_container",
                    legacy.on_primary_container,
                    want.on_primary_container,
                ),
                ("secondary", legacy.secondary, want.secondary),
                (
                    "secondary_container",
                    legacy.secondary_container,
                    want.secondary_container,
                ),
                ("tertiary", legacy.tertiary, want.tertiary),
                (
                    "tertiary_container",
                    legacy.tertiary_container,
                    want.tertiary_container,
                ),
                ("outline", legacy.outline, want.outline),
            ];
            for (name, l, w) in pairs {
                assert!(
                    near(color(l), w),
                    "dark={dark} {name}: {} vs {}",
                    hex_str(color(l)),
                    hex_str(w)
                );
            }
        }
    }

    #[test]
    fn baseline_is_readable() {
        for dark in [false, true] {
            let s = ColorScheme::baseline(dark);
            assert!(contrast_ratio(s.on_surface, s.surface) >= 4.5);
            assert!(contrast_ratio(s.on_primary, s.primary) >= 4.5);
            assert!(contrast_ratio(s.on_primary_container, s.primary_container) >= 4.5);
            assert!(contrast_ratio(s.on_secondary_container, s.secondary_container) >= 4.5);
        }
    }

    #[test]
    fn contrast_levels_raise_contrast() {
        for dark in [false, true] {
            let worst = |c| {
                let s = ColorScheme::from_seed(0xFF67_50A4, SchemeVariant::TonalSpot, dark, c);
                [
                    contrast_ratio(s.on_primary_container, s.primary_container),
                    contrast_ratio(s.on_secondary_container, s.secondary_container),
                    contrast_ratio(s.on_surface, s.surface),
                    contrast_ratio(s.on_primary, s.primary),
                ]
                .into_iter()
                .fold(f32::MAX, f32::min)
            };
            let (std, med, high) = (
                worst(Contrast::Standard),
                worst(Contrast::Medium),
                worst(Contrast::High),
            );
            assert!(
                std >= 4.5 && med >= 4.5 && high >= 7.0,
                "dark={dark}: {std} {med} {high}"
            );
        }
    }

    #[test]
    fn content_schemes_are_readable() {
        let seeds = [
            0xFFE5_1C23, // red
            0xFFFF_EB3B, // yellow
            0xFF00_BCD4, // cyan
            0xFF4C_AF50, // green
            0xFF21_96F3, // blue
            0xFF9C_27B0, // purple
            0xFFFF_9800, // orange
            0xFF80_8080, // mid grey
            0xFF0A_0A0A, // near black
            0xFFF5_F5F5, // near white
            0xFF1B_2A2F, // dark teal
            0xFFFF_00FF, // magenta
            0xFF85_0096, // objectionable tertiary hue
        ];
        for seed in seeds {
            for dark in [false, true] {
                for contrast in [Contrast::Standard, Contrast::Medium, Contrast::High] {
                    let s = ColorScheme::from_seed(seed, SchemeVariant::Content, dark, contrast);
                    let pairs = [
                        ("on_surface/surface", s.on_surface, s.surface),
                        ("on_primary/primary", s.on_primary, s.primary),
                        (
                            "on_primary_container/primary_container",
                            s.on_primary_container,
                            s.primary_container,
                        ),
                        (
                            "on_secondary_container/secondary_container",
                            s.on_secondary_container,
                            s.secondary_container,
                        ),
                    ];
                    for (name, fg, bg) in pairs {
                        let ratio = contrast_ratio(fg, bg);
                        assert!(
                            ratio >= 4.5,
                            "seed {seed:08X} dark={dark} {contrast:?} {name}: {ratio:.2}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_variant_produces_a_scheme() {
        for variant in SchemeVariant::ALL {
            let s = ColorScheme::from_seed(0xFF33_66CC, variant, true, Contrast::Standard);
            assert_ne!(s.surface, s.on_surface, "{variant:?}");
        }
    }

    #[test]
    fn lerp_hits_the_endpoints_and_clamps() {
        let a = ColorScheme::baseline(false);
        let b = ColorScheme::baseline(true);
        assert_eq!(ColorScheme::lerp(&a, &b, 0.0), a);
        assert_eq!(ColorScheme::lerp(&a, &b, 1.0), b);
        let mid = ColorScheme::lerp(&a, &b, 0.5);
        assert_ne!(mid.surface, a.surface);
        assert_ne!(mid.surface, b.surface);
        // Overshoot never wraps.
        let over = ColorScheme::lerp(&a, &b, 1.4);
        assert!(contrast_ratio(over.surface, b.surface) < 1.5);
    }

    #[test]
    fn roles_lists_every_field() {
        assert_eq!(ColorScheme::baseline(true).roles().len(), 45);
    }

    #[test]
    fn contrast_ratio_matches_wcag_extremes() {
        let r = contrast_ratio(Color32::BLACK, Color32::WHITE);
        assert!((r - 21.0).abs() < 0.01, "{r}");
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-4);
    }

    fn solid(rgb: [u8; 3], w: usize, h: usize) -> Vec<u8> {
        (0..w * h)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect()
    }

    #[test]
    fn seed_from_a_coloured_image_is_that_hue() {
        let img = solid([200, 30, 40], 300, 200);
        let seed = seed_from_image(&img, 300, 200).expect("a red seed");
        let (r, g, b) = ((seed >> 16) & 0xFF, (seed >> 8) & 0xFF, seed & 0xFF);
        assert!(r > g + 60 && r > b + 60, "{seed:08X}");
    }

    #[test]
    fn seed_skips_the_grey_background() {
        // Mostly white with a blue patch in the corner (about 10%).
        let (w, h) = (100, 100);
        let mut img = solid([245, 245, 245], w, h);
        for y in 0..30 {
            for x in 0..30 {
                let i = (y * w + x) * 4;
                img[i..i + 3].copy_from_slice(&[20, 60, 220]);
            }
        }
        let seed = seed_from_image(&img, w, h).expect("a blue seed");
        assert!(seed & 0xFF > (seed >> 16) & 0xFF, "{seed:08X}");
    }

    #[test]
    fn seed_is_none_for_grey_and_bad_input() {
        let grey = solid([128, 128, 128], 64, 64);
        assert_eq!(seed_from_image(&grey, 64, 64), None);
        let gradient: Vec<u8> = (0..64 * 64)
            .flat_map(|i| [(i % 256) as u8, (i % 256) as u8, (i % 256) as u8, 255])
            .collect();
        assert_eq!(seed_from_image(&gradient, 64, 64), None);
        assert_eq!(seed_from_image(&[], 0, 0), None);
        assert_eq!(seed_from_image(&grey, 10, 10), None);
        let clear = vec![0u8; 16 * 16 * 4];
        assert_eq!(seed_from_image(&clear, 16, 16), None);
    }
}
