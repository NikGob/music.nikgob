//! Material Symbols Rounded (weight 400, 24 px, outlined and filled).
//!
//! The SVGs live in `assets/icons/{outlined,filled}/` and are drawn in white,
//! so any colour comes from `Image::tint`. [`install`] registers them (and
//! egui's SVG loader) with a context.

use std::borrow::Cow;

use egui::{Color32, Context, Image, ImageSource, Vec2};

/// Declares [`Icon`] and, from the same list, one `fastframe_icons` set per
/// style so the two can never drift apart.
macro_rules! material_icons {
    ($($(#[$meta:meta])* $variant:ident => $file:literal),* $(,)?) => {
        /// An icon the interface draws. Every icon has an outlined and a
        /// filled (selected, toggled-on) form.
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        pub enum Icon {
            $($(#[$meta])* $variant,)*
        }

        mod outlined {
            fastframe_icons::icons! {
                pub(super) enum Set {
                    prefix: "md3e-icon-",
                    directory: "../../../assets/icons/outlined/",
                    $($variant => $file,)*
                }
            }
        }

        mod filled {
            fastframe_icons::icons! {
                pub(super) enum Set {
                    prefix: "md3e-icon-fill-",
                    directory: "../../../assets/icons/filled/",
                    $($variant => $file,)*
                }
            }
        }

        impl Icon {
            /// Every icon, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            /// The Material Symbols name (the SVG's file stem).
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $file,)*
                }
            }

            /// The URI the icon is registered under.
            pub const fn uri(self, filled: bool) -> &'static str {
                if filled {
                    match self {
                        $(Self::$variant => filled::Set::$variant.uri(),)*
                    }
                } else {
                    match self {
                        $(Self::$variant => outlined::Set::$variant.uri(),)*
                    }
                }
            }

            /// The icon's SVG bytes.
            pub const fn bytes(self, filled: bool) -> &'static [u8] {
                if filled {
                    match self {
                        $(Self::$variant => filled::Set::$variant.bytes(),)*
                    }
                } else {
                    match self {
                        $(Self::$variant => outlined::Set::$variant.bytes(),)*
                    }
                }
            }
        }

        /// Registers the icons and egui's image loaders with `ctx`. Call once
        /// at start-up; calling it again is harmless.
        pub fn install(ctx: &Context) {
            egui_extras::install_image_loaders(ctx);
            fastframe_icons::install::<outlined::Set>(ctx);
            fastframe_icons::install::<filled::Set>(ctx);
        }
    };
}

material_icons! {
    Home => "home",
    LibraryMusic => "library_music",
    Download => "download",
    Settings => "settings",
    Search => "search",
    Add => "add",
    Close => "close",
    PlayArrow => "play_arrow",
    Pause => "pause",
    SkipPrevious => "skip_previous",
    SkipNext => "skip_next",
    Shuffle => "shuffle",
    Repeat => "repeat",
    RepeatOne => "repeat_one",
    Favorite => "favorite",
    QueueMusic => "queue_music",
    Lyrics => "lyrics",
    VolumeUp => "volume_up",
    VolumeOff => "volume_off",
    MoreVert => "more_vert",
    MoreHoriz => "more_horiz",
    ExpandMore => "expand_more",
    ExpandLess => "expand_less",
    ChevronLeft => "chevron_left",
    ChevronRight => "chevron_right",
    ArrowBack => "arrow_back",
    ArrowForward => "arrow_forward",
    Check => "check",
    Delete => "delete",
    Edit => "edit",
    PlaylistAdd => "playlist_add",
    Album => "album",
    Person => "person",
    Folder => "folder",
    UploadFile => "upload_file",
    Link => "link",
    Info => "info",
    Sort => "sort",
    GridView => "grid_view",
    ViewList => "view_list",
    /// Stands in for a magnet link (Material Symbols has no magnet).
    AttachFile => "attach_file",
    Equalizer => "equalizer",
    GraphicEq => "graphic_eq",
    CloudDownload => "cloud_download",
    Error => "error",
    Fullscreen => "fullscreen",
    DragIndicator => "drag_indicator",
    MusicNote => "music_note",
}

impl Icon {
    /// Whether the filled form differs from the outlined one. Stroke-only
    /// glyphs (add, close, search, ...) have no fill variant in Material
    /// Symbols; both forms are then the same drawing.
    pub fn has_filled_form(self) -> bool {
        self.bytes(false) != self.bytes(true)
    }

    /// The icon as an egui image source. Wrap it in `egui::Image::new` and
    /// tint it; see [`Self::widget`].
    pub fn image(self, filled: bool) -> ImageSource<'static> {
        ImageSource::Uri(Cow::Borrowed(self.uri(filled)))
    }

    /// A square `size`-point image tinted `tint`.
    pub fn widget(self, filled: bool, tint: Color32, size: f32) -> Image<'static> {
        Image::new(self.image(filled))
            .tint(tint)
            .fit_to_exact_size(Vec2::splat(size))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use egui::{SizeHint, TextureOptions, load::TexturePoll};

    use super::*;

    #[test]
    fn every_icon_has_two_distinct_svgs() {
        let mut uris = HashSet::new();
        for &icon in Icon::ALL {
            for filled in [false, true] {
                let bytes = icon.bytes(filled);
                assert!(bytes.starts_with(b"<svg"), "{icon:?}");
                assert!(
                    uris.insert(icon.uri(filled)),
                    "duplicate {}",
                    icon.uri(filled)
                );
            }
        }
        for icon in [
            Icon::Home,
            Icon::Favorite,
            Icon::PlayArrow,
            Icon::Settings,
            Icon::Folder,
        ] {
            assert!(icon.has_filled_form(), "{icon:?}");
        }
        assert!(!Icon::Add.has_filled_form());
        assert_eq!(uris.len(), Icon::ALL.len() * 2);
    }

    #[test]
    fn icons_are_white_so_tint_is_the_colour() {
        for &icon in Icon::ALL {
            let svg = std::str::from_utf8(icon.bytes(false)).unwrap();
            assert!(svg.contains(r##"fill="#fff""##), "{icon:?}");
        }
    }

    #[test]
    fn icons_rasterize_through_egui() {
        let ctx = Context::default();
        install(&ctx);
        install(&ctx);
        for filled in [false, true] {
            let poll = ctx
                .try_load_texture(
                    Icon::PlayArrow.uri(filled),
                    TextureOptions::LINEAR,
                    SizeHint::Size {
                        width: 48,
                        height: 48,
                        maintain_aspect_ratio: true,
                    },
                )
                .unwrap_or_else(|e| panic!("load failed: {e}"));
            assert!(matches!(poll, TexturePoll::Ready { .. }));
        }
    }

    #[test]
    fn names_match_the_files() {
        assert_eq!(Icon::LibraryMusic.name(), "library_music");
        assert_eq!(Icon::AttachFile.name(), "attach_file");
    }
}
