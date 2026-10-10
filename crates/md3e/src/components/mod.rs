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
pub use slider::{Slider, SliderOutput, Wave, wavy_progress};
pub use toolbar::{ToolbarColor, floating_toolbar};

mod common;
pub(crate) use common::paint_image;
pub mod badge;
pub mod divider;
pub mod switch;
pub mod tooltip;
pub use switch::Switch;
pub use tooltip::{TooltipExt, tooltip};
pub mod textfield;
pub use textfield::{FieldStyle, TextField, TextFieldOutput};
pub mod chip;
pub use chip::{AssistChip, FilterChip, InputChip, InputChipOutput};
pub mod card;
pub use card::{Card, CardKind, CardOutput, MediaCard, MediaCardOutput};
pub mod list;
pub use list::{Leading, ListItem, ListItemOutput, Position, segmented_list};
pub mod snackbar;
pub use snackbar::{SnackbarEvents, SnackbarHost, SnackbarItem};
pub mod dialog;
pub use dialog::{Dialog, DialogAction, DialogOutput, TextInputDialog, TextInputResult};
pub mod menu;
pub use menu::{Menu, MenuItem};
pub mod search;
pub use search::{Search, SearchBar, SearchBarOutput, SearchOutput};
pub mod sheet;
pub use sheet::{BottomSheet, Edge, SheetOutput, SideSheet, SupportingPane};
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
