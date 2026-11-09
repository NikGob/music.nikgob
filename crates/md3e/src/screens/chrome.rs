//! State shared by the floating player, Now Playing and the supporting pane:
//! track menus, the playlist picker, the volume popover, small dialogs and
//! requests the shell carries out (navigation, opening a pane tab).

use std::time::Duration;

use egui::{
    Area, Context, Id, Key, Modifiers, Order, Rect, Response, Sense, Stroke, Ui, UiBuilder, Vec2,
    pos2, vec2,
};
use nikgob_view::{
    Command, LIKED_PLAYLIST, LikeTarget, NpSlot, Route, TrackId, TrackRow, ViewState,
};

use super::{PaneTab, tag_editor::TagEditorUi, visualizer::Viz};
use crate::{
    Elevation, Icon, Theme, TypeRole,
    components::{
        Dialog, IconButton, Menu, MenuItem, Size, TextInputDialog, Radii, anim,
        dialog::TextInputResult, paint,
    },
    tokens::shape,
};

/// An entry of a track's ⋮ menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Like,
    Lyrics,
    Volume,
    Playlist,
    Artist,
    Album,
    Info,
    Tags,
    Replace,
    Visualizer,
    Trash,
    /// A song.link of the track into the clipboard.
    Share,
    /// «Найти песню…» for a clip, «Другая версия…» for another stream
    /// that plays from YouTube (only shown then).
    FindSong,
    /// «Радио по треку»: an endless queue grown from the track.
    Radio,
    /// «Не нравится» (or its undo).
    Dislike,
    /// «Не рекомендовать исполнителя» (or its undo) of the main artist.
    BanArtist,
    /// «В файле другая песня»: the file to the Trash, the track looked for
    /// again (only with a file).
    WrongFile,
    /// «Есть и на альбоме «…»»: the other album of one song on two albums
    /// (only shown then, after «К альбому»).
    AlsoOn,
    /// «Разные песни»: the way out of [`Act::AlsoOn`].
    Apart,
    /// A divider.
    Sep,
}

/// Where a menu was opened, which decides what "show the lyrics" means.
pub enum Where {
    /// The floating player: the shell shows the pane tab.
    Bar,
    /// Now Playing: select its content slot.
    Np,
}

/// What the "В плейлист" picker is for.
#[derive(Clone)]
struct Picker {
    tracks: Vec<TrackId>,
    anchor: Rect,
}

/// Which text-input dialog is open.
#[derive(Clone)]
enum Ask {
    SaveQueue,
    NewPlaylist(Vec<TrackId>),
}

const POPOVER: Vec2 = vec2(52.0, 164.0);

/// Where the wheel turns the volume besides the 🔊 button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelZone {
    /// The floating player: the popover shows what changed.
    Bar,
    /// Now Playing: its own slider shows it.
    Np,
}

#[derive(Default)]
struct Volume {
    /// The 🔊 button as drawn last frame (the hover zone), if shown.
    button: Option<Rect>,
    /// The popover last frame, if it was visible.
    popover: Option<Rect>,
    /// Opened from the ⋮ menu: anchored there until a click outside.
    pinned: Option<Rect>,
    open_until: f64,
    dragging: bool,
    /// The value we just sent, shown until the runtime state catches up.
    shadow: Option<(f32, f64)>,
    /// What a held slider sent last and the state revision it saw then.
    pushed: Option<(f32, u64)>,
    /// Wheel zones and the rects inside them that scroll instead, as drawn
    /// last frame (`zones`) and this frame (`next_*`).
    zones: Vec<(Rect, WheelZone)>,
    blocks: Vec<Rect>,
    next_zones: Vec<(Rect, WheelZone)>,
    next_blocks: Vec<Rect>,
    /// Wheel speed in notches per second (smoothed) and the last notch.
    rate: f32,
    last_notch: f64,
    /// Fractions of a percent not applied yet (touchpads).
    carry: f32,
    /// The popover shown after a wheel turn over the bar, until then.
    flash_until: f64,
    flash_at: Option<Rect>,
}

/// Presentation state outside the page content.
#[derive(Default)]
pub struct Chrome {
    pub visualizer: bool,
    pub viz: Viz,
    pub bands: Vec<f32>,
    /// Now Playing's content, remembered between openings and in prefs.
    pub np_slot: NpSlot,
    pub np_loaded: bool,
    pub np_saved: Option<NpSlot>,
    /// Now Playing covers the shell: the bar under it must not react.
    pub np_covering: bool,
    /// A navigation for the shell (it also closes Now Playing).
    pub nav: Option<Route>,
    /// Show this tab in the shell's pane.
    pub open_pane: Option<PaneTab>,
    /// Open Settings at this row (it also closes Now Playing).
    pub setting: Option<super::pages::SettingId>,
    pub tag: TagEditorUi,
    picker: Option<Picker>,
    ask: Option<Ask>,
    ask_text: String,
    add_after_create: Option<(String, Vec<TrackId>)>,
    trash: Option<(TrackId, String)>,
    vol: Volume,
}

/// "<artist> <album> FLAC": the torrent search for a better copy.
pub fn replace_query(t: &TrackRow) -> String {
    format!("{} {} FLAC", t.artist, t.album.as_deref().unwrap_or(&t.title))
}

/// ⏮: a second press within 400 ms of one that restarted the track goes to
/// the previous track (the runtime restarts first when past 3 s).
pub fn press_previous(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    let key = Id::new("md3e-prev-press");
    let now = ctx.input(|i| i.time);
    let (at, restarted): (f64, bool) = ctx
        .data(|d| d.get_temp(key))
        .unwrap_or((f64::MIN, false));
    let past = state.player.position_now() > Duration::from_secs(3);
    out.push(Command::Previous);
    let restarts = past && !(now - at < 0.4 && restarted);
    ctx.data_mut(|d| d.insert_temp(key, (now, restarts)));
}

impl Chrome {
    // ---- track menu -----------------------------------------------------

    fn item(act: Act, t: &TrackRow, state: &ViewState) -> MenuItem {
        match act {
            Act::Like => MenuItem::new(if t.liked { "Убрать из любимого" } else { "В любимое" })
                .icon(Icon::Favorite)
                .shortcut("Ctrl+L"),
            Act::Lyrics => MenuItem::new("Текст песни").icon(Icon::Lyrics),
            Act::Volume => MenuItem::new("Громкость…")
                .icon(Icon::volume(state.player.volume, state.player.muted)),
            Act::Playlist => MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd),
            Act::Radio => MenuItem::new(super::pages::RADIO_BY_TRACK).icon(Icon::Radio),
            Act::Dislike => super::pages::dislike_item(state.dislikes.track(t.id)),
            Act::BanArtist => super::pages::ban_item(t.artist_id(), &state.dislikes),
            Act::Artist => MenuItem::new("К исполнителю")
                .icon(Icon::Person)
                .enabled(!t.artists.linked().is_empty()),
            Act::Album => MenuItem::new("К альбому")
                .icon(Icon::Album)
                .enabled(t.release_id.is_some()),
            Act::Info => MenuItem::new("Источник и качество").icon(Icon::Info),
            // A stream has no file: no tags to edit; «Скачать» instead of a
            // better copy.
            Act::Tags => MenuItem::new("Редактировать теги…").icon(Icon::Edit).enabled(t.has_local),
            Act::Replace if t.has_local => MenuItem::new("Найти качество лучше…").icon(Icon::Search),
            Act::Replace => MenuItem::new("Скачать").icon(Icon::Download),
            Act::Visualizer => MenuItem::new("Визуализатор")
                .icon(Icon::GraphicEq)
                .shortcut("F12"),
            // Only what is in the library (a file, or a like) can leave it.
            Act::Trash => MenuItem::new("Удалить из библиотеки").icon(Icon::Delete).enabled(t.has_local || t.liked),
            Act::Share => MenuItem::new("Поделиться").icon(Icon::Share),
            Act::FindSong => MenuItem::new(super::find_song::label(t).unwrap_or("Найти песню…")).icon(Icon::MusicNote),
            Act::WrongFile => MenuItem::new(super::pages::WRONG_FILE).icon(Icon::Delete).enabled(t.has_local),
            Act::AlsoOn => {
                let album = state.also_on.get(&t.id).map_or("", |(_, a)| a.as_str());
                MenuItem::new(format!("Есть и на альбоме «{album}»")).icon(Icon::Album)
            }
            Act::Apart => MenuItem::new("Разные песни").icon(Icon::Close),
            Act::Sep => MenuItem::new(""),
        }
    }

    /// A ⋮ menu on `anchor`, right-aligned, with `acts` for `track`.
    #[allow(clippy::too_many_arguments)]
    pub fn track_menu(
        &mut self,
        anchor: &Response,
        salt: &'static str,
        acts: &[Act],
        track: &TrackRow,
        state: &ViewState,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        let mut menu = Menu::new(("md3e-track-menu", salt)).align_right(true);
        let mut picks = Vec::new();
        // A stream has no file to tag, and only what is in the library (a
        // file, or a like) can leave it: those items are left out, with a
        // divider that would end the menu or follow another.
        let shown: Vec<Act> = acts
            .iter()
            .copied()
            .filter(|&a| match a {
                Act::Tags | Act::WrongFile => track.has_local,
                Act::Trash => track.has_local || track.liked,
                Act::FindSong => super::find_song::label(track).is_some(),
                Act::AlsoOn | Act::Apart => state.also_on.contains_key(&track.id),
                _ => true,
            })
            .collect();
        for (i, &a) in shown.iter().enumerate() {
            if a == Act::Sep {
                let next = shown.get(i + 1).is_some_and(|&n| n != Act::Sep);
                if next && i > 0 {
                    menu = menu.divider();
                }
            } else {
                menu = menu.item(Self::item(a, track, state));
                picks.push(a);
            }
        }
        if let Some(i) = menu.attach(anchor) {
            if picks[i] == Act::Artist {
                // Several artists: which one.
                self.nav = super::artist_links::go_to(&anchor.ctx, super::artist_links::library(&track.artists));
            } else if picks[i] == Act::Dislike {
                out.push(super::pages::dislike_command(track.id, &state.dislikes));
            } else if picks[i] == Act::BanArtist {
                out.extend(track.artist_id().map(|a| super::pages::ban_command(a, &state.dislikes)));
            } else if picks[i] == Act::AlsoOn {
                self.nav = state.also_on.get(&track.id).map(|(r, _)| Route::Release(*r));
            } else if picks[i] == Act::Apart {
                out.push(Command::RecordingApart(track.id));
            } else {
                self.apply(picks[i], track, anchor.rect, wh, out);
            }
        }
    }

    pub(super) fn apply(
        &mut self,
        act: Act,
        t: &TrackRow,
        anchor: Rect,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        match act {
            Act::Like => out.push(Command::Like(LikeTarget::Track(t.id), !t.liked)),
            Act::Lyrics | Act::Info => {
                let tab = if act == Act::Lyrics { PaneTab::Lyrics } else { PaneTab::Info };
                match wh {
                    Where::Bar => self.open_pane = Some(tab),
                    Where::Np => self.np_slot = if act == Act::Lyrics { NpSlot::Lyrics } else { NpSlot::Info },
                }
            }
            Act::Volume => self.vol.pinned = Some(anchor),
            Act::Playlist => {
                self.picker = Some(Picker { tracks: vec![t.id], anchor });
            }
            Act::Artist => self.nav = t.artist_id().map(Route::Artist),
            Act::Album => self.nav = t.release_id.map(Route::Release),
            Act::Tags => out.push(Command::OpenTagEditor(t.id)),
            Act::Replace if t.has_local => out.push(Command::SearchTorrents(replace_query(t))),
            Act::Replace => out.push(Command::DownloadTracks(vec![t.id])),
            Act::Visualizer => self.visualizer = true,
            Act::Trash => self.trash = Some((t.id, t.title.clone())),
            Act::Share => out.push(Command::Share(nikgob_view::ShareTarget::Track(t.id))),
            Act::FindSong => out.push(Command::FindSong { track: t.id, query: None }),
            Act::WrongFile => out.push(Command::WrongFile(t.id)),
            Act::Radio => out.push(super::pages::radio_by_track(t.id)),
            // Handled where the marks are known.
            Act::Dislike | Act::BanArtist | Act::AlsoOn | Act::Apart | Act::Sep => {}

        }
    }

    /// Opens the "Сохранить как плейлист" name dialog.
    pub fn ask_save_queue(&mut self, default: &str) {
        self.ask_text = default.to_owned();
        self.ask = Some(Ask::SaveQueue);
    }

    // ---- overlays -------------------------------------------------------

    /// Menus, dialogs and popovers that float above the screens.
    pub fn overlays(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        self.picker_ui(ctx, state, out);
        self.dialogs(ctx, state, out);
        self.tag.show(ctx, state, out);
        self.volume_ui(ctx, state, out);
    }

    fn picker_ui(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let Some(p) = self.picker.clone() else { return };
        let user: Vec<_> = state.library.playlists.iter().filter(|p| !p.pinned).collect();
        let mut menu = Menu::new("md3e-playlist-picker")
            .item(MenuItem::new("Любимые").icon(Icon::Favorite));
        for pl in &user {
            menu = menu.item(MenuItem::new(&pl.name).icon(Icon::QueueMusic));
        }
        menu = menu
            .divider()
            .item(MenuItem::new("Новый плейлист…").icon(Icon::Add))
            .align_right(true);
        let mut open = true;
        let picked = menu.popup(ctx, p.anchor, &mut open);
        match picked {
            Some(0) => out.push(Command::AddToPlaylist(LIKED_PLAYLIST, p.tracks.clone())),
            Some(i) if i <= user.len() => {
                out.push(Command::AddToPlaylist(user[i - 1].id, p.tracks.clone()));
            }
            Some(_) => {
                self.ask_text.clear();
                self.ask = Some(Ask::NewPlaylist(p.tracks.clone()));
            }
            None => {}
        }
        if picked.is_some() || !open {
            self.picker = None;
        }
    }

    fn dialogs(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        if let Some(ask) = self.ask.clone() {
            let mut open = true;
            let dialog = match &ask {
                Ask::SaveQueue => {
                    TextInputDialog::new("md3e-save-queue", "Сохранить очередь", "Название")
                        .icon(Icon::PlaylistAdd)
                        .buttons("Сохранить", "Отмена")
                }
                Ask::NewPlaylist(_) => {
                    TextInputDialog::new("md3e-new-playlist", "Новый плейлист", "Название")
                        .icon(Icon::PlaylistAdd)
                        .buttons("Создать", "Отмена")
                }
            }
            .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned()));
            match dialog.show(ctx, &mut open, &mut self.ask_text) {
                Some(TextInputResult::Confirmed) => {
                    let name = self.ask_text.trim().to_owned();
                    match ask {
                        Ask::SaveQueue => out.push(Command::SaveQueueAsPlaylist(name)),
                        Ask::NewPlaylist(tracks) => {
                            out.push(Command::CreatePlaylist(name.clone()));
                            self.add_after_create = Some((name, tracks));
                        }
                    }
                    self.ask = None;
                }
                Some(TextInputResult::Cancelled) => self.ask = None,
                None if !open => self.ask = None,
                None => {}
            }
        }
        // Tracks waiting for the playlist we just created.
        if let Some((name, tracks)) = self.add_after_create.clone()
            && let Some(p) = state
                .library
                .playlists
                .iter()
                .find(|p| p.name == name && !p.pinned)
        {
            out.push(Command::AddToPlaylist(p.id, tracks));
            self.add_after_create = None;
        }

        if let Some((id, title)) = self.trash.clone() {
            let mut open = true;
            let trash = state.prefs.use_trash;
            let r = Dialog::new("md3e-confirm-trash", if trash { "Удалить из библиотеки?" } else { "Удалить навсегда?" })
                .icon(Icon::Delete)
                .supporting(if trash {
                    format!("«{title}» вместе с файлами уйдёт в корзину на 30 дней.")
                } else {
                    format!("«{title}»: файлы будут стёрты с диска. Корзина выключена, отменить нельзя.")
                })
                .action("Отмена")
                .action(if trash { "Удалить" } else { "Удалить навсегда" })
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::TrashTrack(id));
            }
            if r.action.is_some() || r.dismissed || !open {
                self.trash = None;
            }
        }
    }

    // ---- volume ---------------------------------------------------------

    /// The volume to show: what we just sent, until the runtime catches up.
    pub fn volume_now(&self, ctx: &Context, state: &ViewState) -> f32 {
        match self.vol.shadow {
            Some((v, t)) if anim::now(ctx) - t < 0.6 => v,
            _ => state.player.volume,
        }
    }

    fn send_volume(&mut self, ctx: &Context, v: f32, out: &mut Vec<Command>) {
        let v = v.clamp(0.0, 1.0);
        self.vol.shadow = Some((v, anim::now(ctx)));
        out.push(Command::SetVolume(v));
    }

    /// Sends the volume of a held slider, which reports its value every
    /// frame: only a value the runtime does not have or was not just sent.
    pub fn push_volume(&mut self, state: &ViewState, v: f32, out: &mut Vec<Command>) {
        let p = &state.player;
        let known = v == p.volume || self.vol.pushed == Some((v, state.revision));
        if known && !p.muted {
            return;
        }
        self.vol.pushed = Some((v, state.revision));
        out.push(Command::SetVolume(v));
    }

    /// Reports where the 🔊 button is (the hover zone of the popover).
    /// The volume popover is on screen (its button's tooltip would cover it).
    pub fn volume_open(&self) -> bool {
        self.vol.popover.is_some()
    }

    pub fn set_volume_button(&mut self, rect: Option<Rect>) {
        self.vol.button = rect;
    }

    /// The wheel turns the volume over `rect` (from this frame on: zones
    /// are read a frame late, before anything else uses the wheel).
    pub fn volume_zone(&mut self, rect: Rect, zone: WheelZone) {
        self.vol.next_zones.push((rect, zone));
    }

    /// Inside a volume zone, `rect` scrolls as usual (lists, lyrics).
    pub fn wheel_block(&mut self, rect: Rect) {
        self.vol.next_blocks.push(rect);
    }

    /// The wheel over the 🔊 button, its popover, the floating player or
    /// Now Playing changes the volume by whole percents: 1 % a notch when
    /// turned slowly, up to 5 % when spun fast. Runs first in the frame so
    /// the page underneath does not scroll too.
    pub fn wheel(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let zones = std::mem::take(&mut self.vol.next_zones);
        let blocks = std::mem::take(&mut self.vol.next_blocks);
        self.vol.zones = zones;
        self.vol.blocks = blocks;
        let Some(p) = ctx.pointer_hover_pos() else { return };
        let on_button = self.vol.button.is_some_and(|b| b.contains(p))
            || self.vol.popover.is_some_and(|r| r.contains(p));
        let zone = if on_button {
            None
        } else if self.vol.blocks.iter().any(|b| b.contains(p)) {
            return;
        } else {
            match self.vol.zones.iter().rev().find(|(r, _)| r.contains(p)) {
                Some(&(_, z)) => Some(z),
                None => return,
            }
        };
        // Raw wheel events, not the smoothed scroll: whatever the platform's
        // line height or scroll smoothing.
        let (notches, points): (f32, f32) = ctx.input(|i| {
            i.raw.events.iter().fold((0.0, 0.0), |(n, pt), e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => (n + delta.y, pt),
                    egui::MouseWheelUnit::Page => (n + delta.y * 5.0, pt),
                    egui::MouseWheelUnit::Point => (n, pt + delta.y),
                },
                _ => (n, pt),
            })
        });
        // The page underneath must not scroll while the pointer is here.
        ctx.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        if notches == 0.0 && points == 0.0 {
            return;
        }
        let now = anim::now(ctx);
        let percent = if notches != 0.0 {
            let dt = (now - self.vol.last_notch).clamp(1.0 / 240.0, 1.0) as f32;
            let rate = notches.abs() / dt;
            self.vol.rate = if dt >= 1.0 { rate.min(8.0) } else { self.vol.rate * 0.6 + rate * 0.4 };
            self.vol.last_notch = now;
            let per_notch = (1.0 + (self.vol.rate - 8.0) / 6.0).clamp(1.0, 5.0).round();
            notches * per_notch
        } else {
            // Touchpads: 1 % per 12 points.
            points / 12.0
        };
        self.vol.carry += percent;
        let whole = self.vol.carry.trunc();
        self.vol.carry -= whole;
        if zone == Some(WheelZone::Bar) {
            self.vol.flash_until = now + 1.2;
            self.vol.flash_at = Some(Rect::from_center_size(p, vec2(8.0, 8.0)));
        }
        if whole == 0.0 {
            return;
        }
        let current = (self.volume_now(ctx, state) * 100.0).round();
        self.send_volume(ctx, (current + whole) / 100.0, out);
    }

    fn volume_ui(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let theme = Theme::get(ctx);
        let s = theme.scheme;
        let now = anim::now(ctx);
        let flashing = now < self.vol.flash_until;
        let anchor = self
            .vol
            .pinned
            .or(self.vol.button)
            .or(self.vol.flash_at.filter(|_| flashing));
        let ptr = ctx.pointer_hover_pos();

        let mut wanted = false;
        if let Some(a) = anchor {
            let zone = self.vol.popover.map_or(a, |p| a.union(p)).expand(2.0);
            let inside = ptr.is_some_and(|p| zone.contains(p));
            if self.vol.pinned.is_some() {
                let press_out = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer.interact_pos().is_some_and(|p| !zone.contains(p))
                });
                let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
                if press_out || esc {
                    self.vol.pinned = None;
                }
                wanted = self.vol.pinned.is_some();
            } else {
                if inside || self.vol.dragging {
                    self.vol.open_until = now + 0.25;
                }
                wanted = now < self.vol.open_until || flashing;
                if wanted && !inside {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
        let alpha = anim::fraction(
            ctx,
            Id::new("md3e-volume-popover"),
            f32::from(u8::from(wanted)),
            theme.motion.fast_effects,
        )
        .clamp(0.0, 1.0);
        let Some(a) = anchor.filter(|_| alpha > 0.01) else {
            self.vol.popover = None;
            self.vol.dragging = false;
            return;
        };

        // Over the bar's own sound button the pill needs no mute button of
        // its own: that one right below already mutes.
        let own_mute = self.vol.pinned.is_some() || anchor != self.vol.button;
        let size = if own_mute { POPOVER } else { POPOVER - vec2(0.0, 36.0) };
        let screen = ctx.content_rect().shrink(8.0);
        let x = (a.center().x - size.x * 0.5)
            .clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
        let above = a.top() - 8.0 - size.y;
        let y = if above >= screen.top() { above } else { a.bottom() + 8.0 };
        let rect = Rect::from_min_size(pos2(x, y + (1.0 - alpha) * 8.0), size);
        self.vol.popover = Some(rect);

        let volume = if state.player.muted { 0.0 } else { self.volume_now(ctx, state) };
        let mut new_value = None;
        let mut toggle_mute = false;
        let mut dragging = false;
        Area::new(Id::new("md3e-volume-area"))
            .fade_in(false)
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                let radii = Radii::all(shape::EXTRA_LARGE);
                paint::shadow(ui.painter(), r, radii, Elevation::Level2);
                paint::container(ui.painter(), r, radii, s.surface_container_high, Stroke::NONE);
                ui.interact(r, Id::new("md3e-volume-bg"), Sense::click());

                let label = format!("{}", (volume * 100.0).round() as i32);
                let g = ui.painter().layout_no_wrap(
                    label,
                    TypeRole::LabelMedium.font_id(),
                    s.on_surface_variant,
                );
                ui.painter().galley(
                    pos2(r.center().x - g.size().x * 0.5, r.top() + 11.0),
                    g,
                    s.on_surface_variant,
                );

                let track = Rect::from_min_max(
                    pos2(r.center().x - 18.0, r.top() + 34.0),
                    pos2(r.center().x + 18.0, r.bottom() - if own_mute { 50.0 } else { 14.0 }),
                );
                let resp = ui.interact(track, Id::new("md3e-volume-slider"), Sense::click_and_drag());
                dragging = resp.dragged();
                if let Some(p) = resp.interact_pointer_pos()
                    && (resp.dragged() || resp.is_pointer_button_down_on() || resp.clicked())
                {
                    new_value = Some(((track.bottom() - p.y) / track.height()).clamp(0.0, 1.0));
                }
                paint_vslider(ui, track, new_value.unwrap_or(volume), dragging);

                if !own_mute {
                    return;
                }
                let btn = Rect::from_center_size(pos2(r.center().x, r.bottom() - 25.0), vec2(40.0, 40.0));
                let mute = ui
                    .scope_builder(UiBuilder::new().max_rect(btn), |ui| {
                        ui.add(
                            IconButton::new(Icon::volume(volume, state.player.muted))
                                .size(Size::S)
                                .tooltip("Без звука")
                                .id_salt("md3e-volume-mute"),
                        )
                    })
                    .inner;
                toggle_mute = mute.clicked();
            });
        self.vol.dragging = dragging;
        if let Some(v) = new_value
            && (v - volume).abs() > 0.002
        {
            self.send_volume(ctx, v, out);
        }
        if toggle_mute {
            out.push(Command::ToggleMute);
        }
    }
}

/// The vertical Expressive slider: 14 wide track, 36×4 handle with 5 gaps.
fn paint_vslider(ui: &Ui, rect: Rect, value: f32, pressed: bool) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let w = 14.0;
    let gap = 5.0;
    let handle_h = anim::f32(
        ui.ctx(),
        Id::new("md3e-volume-handle"),
        if pressed { 2.0 } else { 4.0 },
        theme.motion.fast_spatial,
    );
    let cx = rect.center().x;
    let y = rect.bottom() - value.clamp(0.0, 1.0) * rect.height();
    let top = Rect::from_min_max(
        pos2(cx - w * 0.5, rect.top()),
        pos2(cx + w * 0.5, (y - gap - handle_h * 0.5).max(rect.top())),
    );
    if top.height() > 0.5 {
        paint::container(
            ui.painter(),
            top,
            Radii([shape::FULL, shape::FULL, 2.0, 2.0]),
            s.secondary_container,
            Stroke::NONE,
        );
    }
    let bottom = Rect::from_min_max(
        pos2(cx - w * 0.5, (y + gap + handle_h * 0.5).min(rect.bottom())),
        pos2(cx + w * 0.5, rect.bottom()),
    );
    if bottom.height() > 0.5 {
        paint::container(
            ui.painter(),
            bottom,
            Radii([2.0, 2.0, shape::FULL, shape::FULL]),
            s.primary,
            Stroke::NONE,
        );
    }
    paint::container(
        ui.painter(),
        Rect::from_center_size(pos2(cx, y), vec2(rect.width(), handle_h)),
        Radii::all(shape::FULL),
        s.primary,
        Stroke::NONE,
    );
}
