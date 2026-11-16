//! Material 3 Expressive foundations for egui: colour roles generated from a
//! seed (or from album art), the type scale on Roboto Flex, Material Symbols
//! icons, shape/state/elevation/size-class/motion tokens, and a [`Theme`]
//! stored in the egui context.
//!
//! Start-up, once per context:
//!
//! ```ignore
//! let mut fonts = egui::FontDefinitions::default();
//! nikgob_md3e::typography::install_fonts(&mut fonts);
//! ctx.set_fonts(fonts);
//! nikgob_md3e::icons::install(&ctx);
//! Theme::new(ColorScheme::from_seed(seed, SchemeVariant::Content, true, Contrast::Standard), true)
//!     .install(&ctx);
//! ```
//!
//! Components and screens are built on top of this layer elsewhere.

pub mod color;
pub mod components;
pub mod covers;
pub mod fit;
pub mod screens;
pub mod icons;
#[cfg(target_os = "android")]
mod no_rfd;
pub mod prof;
pub mod shot;
pub mod theme;
pub mod tokens;
pub mod typography;

pub use color::{
    ColorScheme, Contrast, SchemeVariant, contrast_ratio, lerp_color, over, seed_from_image,
    with_opacity,
};
pub use icons::Icon;
pub use nikgob_motion::{Animated, Spring};
pub use theme::Theme;
pub use tokens::{Elevation, Motion, SizeClass, size_class};
pub use typography::{TypeRole, TypeStyle};
