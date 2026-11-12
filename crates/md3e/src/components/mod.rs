//! MD3E components for egui. Each draws only through theme roles and
//! animates through [`anim`] springs.

pub mod anim;
pub mod button;
pub mod group;
pub mod paint;
pub mod rail;
pub mod slider;
pub mod toolbar;

pub use button::{Button, IconButton, IconWidth, Shape, Size, Style};
pub use group::{ButtonGroup, ConnectedGroup, Item};
pub use paint::Radii;
pub use rail::{NavRail, RailItem};
pub use slider::{Slider, SliderOutput, Wave, wavy_progress, wavy_progress_indeterminate, wavy_progress_on};
pub use toolbar::{ToolbarColor, floating_toolbar};

mod common;
pub(crate) use common::{paint_image, paint_image_shaped, paint_image_zoomed, paint_shape_avatar, paint_shape_avatar_turned};

/// Filtering for photos drawn smaller than their file (covers, avatars):
/// trilinear mipmaps, so a 1200 px cover at 48 dp does not alias.
pub const PHOTO_TEXTURE: egui::TextureOptions =
    egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
pub mod badge;
pub mod divider;
pub mod checkbox;
pub mod switch;
pub use checkbox::Checkbox;
pub mod tooltip;
pub use switch::Switch;
pub use tooltip::{TooltipExt, tooltip};
pub mod textfield;
pub use textfield::{FieldStyle, TextField, TextFieldOutput};
pub mod chip;
pub use chip::{AssistChip, FilterChip, InputChip, InputChipOutput};
pub mod card;
pub use card::{Card, CardKind, CardOutput, CoverStatus, MediaCard, MediaCardOutput};
pub mod links;
pub use links::{LinkLabel, interact_links, link_label, link_paragraph};
pub mod list;
pub use list::{Leading, ListItem, ListItemOutput, Position, segmented_list};
pub mod scrollbars;
pub mod snackbar;
pub use snackbar::{SnackbarEvents, SnackbarHost, SnackbarItem, SnackbarKind};
pub mod dialog;
pub use dialog::{Dialog, DialogAction, DialogOutput, TextInputDialog, TextInputResult};
pub mod menu;
pub use menu::{Menu, MenuItem};
pub mod search;
pub use search::{Search, SearchBar, SearchBarOutput, SearchOutput};
pub mod sheet;
pub use sheet::{BottomSheet, Edge, SheetOutput, SideSheet, SupportingPane};
pub mod modal_window;
pub use modal_window::ModalWindow;
pub mod fab;
pub use fab::{Fab, FabColor, FabMenu, FabMenuItem, FabMenuOutput, FabSize};
pub mod loading;
pub use loading::LoadingIndicator;
pub mod app_bar;
pub use app_bar::{AppBarOutput, FlexibleAppBar, SmallAppBar};
pub use divider::Divider;
#[cfg(test)]
mod smoke;
pub mod equalizer;
pub use equalizer::Equalizer;
pub mod split_button;
pub use split_button::{SplitButton, SplitOutput};
pub mod button_row;
pub use button_row::{ButtonRow, Member, RowOutput};
