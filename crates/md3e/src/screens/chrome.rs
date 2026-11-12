//! State shared by the floating player, Now Playing and the supporting pane:
//! track menus, the playlist picker, the volume popover, small dialogs and
//! requests the shell carries out (navigation, opening a pane tab).

use std::time::Duration;

use egui::{
    Area, Context, Id, Key, Modifiers, Order, Rect, Response, Sense, Stroke, Ui, UiBuilder, Vec2,
    pos2, vec2,
};
use nikgob_view::{Command, LikeTarget, NpSlot, Route, TrackRow, ViewState};

use super::{
    PaneTab,
    pages::{TrackAct, widgets::Row},
    tag_editor::TagEditorUi,
    visualizer::Viz,
};
use crate::{
    Elevation, Icon, Theme, TypeRole,
    components::{
        IconButton, Menu, MenuItem, Size, TextInputDialog, Radii, anim,
        dialog::TextInputResult, paint,
    },
    tokens::shape,
};

/// A row of the playing track's ⋮ that only the player has: what does not
/// fit in the bar's pill, the visualizer, the lyrics editor, the delete.
/// The track's own actions are [`TrackAct`] from the one list of every
/// track menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Like,
    Lyrics,
    Volume,
    Visualizer,
    /// «Изменить текст…»: the editor of the shown lyrics.
    EditLyrics,
    Trash,
    /// «Включить с YouTube» (the developer profile only).
    Youtube,
    /// «Таймер сна ▸»: the player puts its submenu of choices here.
    SleepMenu,
    /// A choice of «Таймер сна».
    Sleep(nikgob_view::SleepChoice),
}

/// What a row of the playing track's ⋮ stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerAct {
    /// The player's own row: before the track's, or in its submenus.
    Own(Act),
    /// A row of [`super::pages::library_track_rows`].
    Track(TrackAct),
}

/// Where a menu was opened, which decides what "show the lyrics" means.
pub enum Where {
    /// The floating player: the shell shows the pane tab.
    Bar,
    /// Now Playing: select its content slot.
    Np,
}

/// Which text-input dialog is open.
#[derive(Clone)]
enum Ask {
    SaveQueue,
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
    ask: Option<Ask>,
    ask_text: String,
    vol: Volume,
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
            Act::Like => MenuItem::new(super::pages::like_label(t.liked))
                .icon(Icon::Favorite)
                .selected(t.liked)
                .shortcut("Ctrl+L"),
            Act::Lyrics => MenuItem::new("Текст песни").icon(Icon::Lyrics),
            Act::Volume => MenuItem::new("Громкость…")
                .icon(Icon::volume(state.player.volume, state.player.muted)),
            Act::Visualizer => MenuItem::new("Визуализатор")
                .icon(Icon::GraphicEq)
                .shortcut("F12"),
            Act::EditLyrics => MenuItem::new("Изменить текст…").icon(Icon::Lyrics),
            // Only what is in the library (a file, or a like) is offered.
            Act::Trash => MenuItem::new("Удалить из библиотеки").icon(Icon::Delete),
            Act::Youtube => MenuItem::new("Включить с YouTube")
                .icon(Icon::PlayArrow)
                .shortcut("Y"),
            Act::SleepMenu => MenuItem::new(super::sleep_ui::MENU_LABEL).icon(Icon::Bedtime),
            Act::Sleep(c) => MenuItem::new(super::sleep_ui::choice_label(c, nikgob_view::SleepContext::of(&state.player))),
        }
    }

    /// The rows of the playing track's ⋮: the player's own rows `top`, then
    /// the one list of a library track ([`super::pages::library_track_rows`])
    /// without «Сыграть следующим» / «В очередь» (it is playing). Its
    /// sound is one row, «Скорость и эквалайзер» (the sound window on its
    /// first tab), and after it, when `top` names it, «Визуализатор»; its
    /// «Файл» has «Изменить текст…» after the tags while the shown lyrics
    /// are this track's and can be edited, and the delete last when `trash`
    /// and the track is in the library.
    pub fn player_rows(ctx: &Context, t: &TrackRow, state: &ViewState, top: &[Act], info: bool, trash: bool) -> Vec<Row<PlayerAct>> {
        use super::pages::sound;
        // The developer profile's tracks have no files: the real song first.
        let youtube = nikgob_view::dev::profile().then_some(Act::Youtube);
        let own = |a: Act| Row::Item(PlayerAct::Own(a), Self::item(a, t, state));
        let mut rows: Vec<Row<PlayerAct>> = youtube
            .iter()
            .chain(top)
            .filter(|a| !matches!(a, Act::Visualizer | Act::SleepMenu))
            .map(|&a| own(a))
            .collect();
        // «Таймер сна ▸» after the player's own rows.
        if top.contains(&Act::SleepMenu) {
            rows.push(super::sleep_ui::submenu(state, |c| PlayerAct::Own(Act::Sleep(c))));
        }
        rows.push(Row::Divider);
        // The frame's index: the bar and Now Playing build these rows every
        // frame for their ⋮.
        let wish = super::pages::widgets::WishIndex::of(ctx, &state.downloads.wishlist);
        let of = super::pages::LibraryRowsOf {
            marks: &state.dislikes,
            info,
            also: state.also_on.get(&t.id),
            torrents: state.prefs.prowlarr_configured,
            wished: (!t.has_local).then(|| wish.of_track(Some(t.id), t.release_id)).flatten(),
        };
        for r in super::pages::library_track_rows(t, of) {
            if !matches!(r, Row::Item(TrackAct::Next | TrackAct::Queue, _)) {
                rows.push(r.map(&mut PlayerAct::Track));
            }
        }
        // The keys are the playing track's: only its own menu names them.
        let (act, keys) = match sound::first_tab(state) {
            sound::Tab::Speed => (TrackAct::Speed, "S, E"),
            sound::Tab::Equalizer => (TrackAct::Equalizer, "E"),
        };
        let at = super::pages::sound_at(&rows);
        let item = MenuItem::new("Скорость и эквалайзер").icon(Icon::Tune).shortcut(keys);
        rows.insert(at, Row::Item(PlayerAct::Track(act), item));
        if top.contains(&Act::Visualizer) {
            rows.insert(at + 1, own(Act::Visualizer));
        }
        if let Some(file) = super::pages::widgets::sub_rows_mut(&mut rows, super::pages::FILE) {
            if state.lyrics.track == Some(t.id) && super::lyrics_ui::can_edit(state) {
                let at = file
                    .iter()
                    .position(|r| matches!(r, Row::Item(PlayerAct::Track(TrackAct::Fetch), _)))
                    .map_or(file.len(), |i| i + 1);
                file.insert(at, own(Act::EditLyrics));
            }
            if trash && super::pages::can_trash(t) {
                file.push(Row::Divider);
                file.push(own(Act::Trash));
            }
        }
        // Nothing that leads back to the page on screen (none under Now
        // Playing, which covers it).
        let also = state.also_on.get(&t.id).map(|(r, _)| *r);
        let rows = super::pages::here(ctx).trim(rows, t, also, |a| match a {
            PlayerAct::Track(a) => Some(a),
            PlayerAct::Own(_) => None,
        });
        // «Меню трека»: the bar's own rows and the delete stay as they are.
        super::pages::placed(rows, &state.prefs.track_menu, |a| match a {
            PlayerAct::Track(a) => a.menu_item(t.has_local),
            PlayerAct::Own(Act::Visualizer) => Some(nikgob_view::TrackMenuItem::Visualizer),
            PlayerAct::Own(Act::EditLyrics) => Some(nikgob_view::TrackMenuItem::LyricsEdit),
            PlayerAct::Own(_) => None,
        })
    }

    /// A ⋮ menu of `rows` ([`Self::player_rows`]) on `anchor`, right-aligned.
    #[allow(clippy::too_many_arguments)]
    pub fn track_menu(
        &mut self,
        anchor: &Response,
        salt: &'static str,
        rows: Vec<Row<PlayerAct>>,
        track: &TrackRow,
        state: &ViewState,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        let (menu, picks) = super::pages::widgets::menu_of(("md3e-track-menu", salt), rows);
        if let Some(act) = menu.align_right(true).attach(anchor).and_then(|i| picks.get(i).copied()) {
            self.pick(&anchor.ctx, act, track, state, anchor.rect, wh, out);
        }
    }

    /// The same menu of `rows` on a right click on `row` (the bar's track,
    /// the queue's playing row); `rows` is built only while it shows.
    #[allow(clippy::too_many_arguments)]
    pub fn track_context_menu(
        &mut self,
        row: &Response,
        salt: &'static str,
        rows: impl FnOnce() -> Vec<Row<PlayerAct>>,
        track: &TrackRow,
        state: &ViewState,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        let key = ("md3e-track-ctx", salt);
        if Menu::context_idle(row, key) {
            return;
        }
        let (menu, picks) = super::pages::widgets::menu_of(key, rows());
        if let Some(act) = menu.context_menu(row).and_then(|i| picks.get(i).copied()) {
            let at = Rect::from_center_size(row.ctx.pointer_latest_pos().unwrap_or(row.rect.center()), vec2(1.0, 1.0));
            self.pick(&row.ctx, act, track, state, at, wh, out);
        }
    }

    /// Does what a row of the playing track's menu stands for.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn pick(
        &mut self,
        ctx: &Context,
        act: PlayerAct,
        track: &TrackRow,
        state: &ViewState,
        anchor: Rect,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        match act {
            PlayerAct::Own(Act::EditLyrics) => super::lyrics_ui::open_editor(ctx, state),
            PlayerAct::Own(a) => self.apply(ctx, a, track, anchor, wh, out),
            PlayerAct::Track(a) => self.apply_track(ctx, a, track, state, anchor, wh, out),
        }
    }

    pub(super) fn apply(
        &mut self,
        ctx: &Context,
        act: Act,
        t: &TrackRow,
        anchor: Rect,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        match act {
            Act::Like => out.push(Command::Like(LikeTarget::Track(t.id), !t.liked)),
            Act::Lyrics => self.show(PaneTab::Lyrics, wh),
            Act::Volume => self.vol.pinned = Some(anchor),
            Act::Visualizer => self.visualizer = true,
            // Opened by the menu itself ([`Self::track_menu`]).
            Act::EditLyrics => {}
            Act::Trash => super::pages::request_delete(ctx, super::pages::Doomed::Tracks(vec![t.id]), format!("«{}»", t.title), false),
            Act::Youtube => out.push(Command::PlayFromYoutube),
            Act::Sleep(c) => out.push(Command::SleepTimer(c)),
            // A submenu, never picked itself.
            Act::SleepMenu => {}
        }
    }

    /// «Текст песни» / «Источник и качество»: the bar shows the shell's
    /// pane tab, Now Playing selects its content slot.
    fn show(&mut self, tab: PaneTab, wh: Where) {
        match wh {
            Where::Bar => self.open_pane = Some(tab),
            Where::Np => self.np_slot = if tab == PaneTab::Lyrics { NpSlot::Lyrics } else { NpSlot::Info },
        }
    }

    /// Does a track action of the playing track (as a library row does).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_track(
        &mut self,
        ctx: &Context,
        act: TrackAct,
        t: &TrackRow,
        state: &ViewState,
        anchor: Rect,
        wh: Where,
        out: &mut Vec<Command>,
    ) {
        use TrackAct as A;
        match act {
            A::Next => out.push(Command::PlayNext(vec![t.id])),
            A::Queue => out.push(Command::Enqueue(vec![t.id])),
            A::Playlist => super::pages::request_tracks_to_playlist(ctx, vec![t.id], Some(anchor)),
            A::ToLibrary => out.push(Command::TrackInLibrary { track: t.id, on: true }),
            A::OutOfLibrary => out.push(Command::TrackInLibrary { track: t.id, on: false }),
            A::Radio => out.push(super::pages::radio_by_track(t.id)),
            A::Album => self.nav = t.release_id.map(Route::Release),
            A::AlsoOn => self.nav = state.also_on.get(&t.id).map(|(r, _)| Route::Release(*r)),
            // For good: asked first.
            A::Apart => {
                if let Some((_, album)) = state.also_on.get(&t.id) {
                    super::pages::request_apart(ctx, t.id, &t.title, album);
                }
            }
            // Several artists: which one (not the one whose page is on
            // screen under the bar).
            A::Artist => {
                let here = match wh {
                    Where::Np => super::pages::Here::default(),
                    Where::Bar => super::pages::here(ctx),
                };
                self.nav = super::artist_links::go_to(ctx, here.artists(&t.artists));
            }
            // Now Playing's sheet; the side pane for the playing track when
            // it is on; any other track's in a window of its own.
            A::Info => match wh {
                Where::Np => self.show(PaneTab::Info, Where::Np),
                Where::Bar if super::pages::is_playing(state, t.id) && PaneTab::Info.on(&state.prefs) => {
                    self.show(PaneTab::Info, Where::Bar);
                }
                Where::Bar => super::info_window::open(ctx, t.id, Some(t.clone()), out),
            },
            A::Share => out.push(Command::Share(nikgob_view::ShareTarget::Track(t.id))),
            // The playing track's own speed (a queue row opens its own).
            A::Speed => super::pages::sound::open(ctx, super::pages::sound::Tab::Speed, None),
            A::Equalizer => super::pages::sound::open(ctx, super::pages::sound::Tab::Equalizer, None),
            A::Fetch if t.has_local => out.push(Command::OpenTagEditor(t.id)),
            A::Fetch => out.push(Command::DownloadTracks(vec![t.id])),
            // The whole album the track is from, the files it lacks.
            A::AlbumDownload => out.extend(t.release_id.map(Command::DownloadRelease)),
            // A better copy through the download sources, as the wishlist
            // looks; no file: the indexer's search page, by hand.
            A::Better => out.push(Command::FindBetter(t.id)),
            A::PickRelease => out.push(super::pages::pick_release_command(t)),
            A::ToMp3 => out.push(Command::FakeToMp3(t.id)),
            A::FindSong => out.push(Command::FindSong { track: t.id, query: None }),
            A::WrongFile => out.push(Command::WrongFile(t.id)),
            A::Dislike => out.push(super::pages::dislike_command(t.id, &state.dislikes)),
            A::Ban => out.extend(t.artist_id().map(|a| super::pages::ban_command(a, &state.dislikes))),
            // A list's rows: not in the player's menu.
            A::Remove | A::Trash | A::Purge => {}
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
        self.dialogs(ctx, out);
        self.tag.show(ctx, state, out);
        self.volume_ui(ctx, state, out);
    }

    fn dialogs(&mut self, ctx: &Context, out: &mut Vec<Command>) {
        if let Some(ask) = self.ask.clone() {
            let mut open = true;
            let dialog = match &ask {
                Ask::SaveQueue => {
                    TextInputDialog::new("md3e-save-queue", "Сохранить очередь", "Название")
                        .icon(Icon::PlaylistAdd)
                        .buttons("Сохранить", "Отмена")
                }
            }
            .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned()));
            match dialog.show(ctx, &mut open, &mut self.ask_text) {
                Some(TextInputResult::Confirmed) => {
                    let name = self.ask_text.trim().to_owned();
                    match ask {
                        Ask::SaveQueue => out.push(Command::SaveQueueAsPlaylist(name)),
                    }
                    self.ask = None;
                }
                Some(TextInputResult::Cancelled) => self.ask = None,
                None if !open => self.ask = None,
                None => {}
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
        // A dialog over Now Playing (the lyrics window, any other) scrolls
        // its own content: the wheel is its, not the volume's. Beside the
        // window, over its scrim, the wheel is the volume's as without it.
        if ctx.layer_id_at(p).is_some_and(|l| l.order >= egui::Order::Tooltip && !crate::components::ModalWindow::scrim_at(ctx, l, p))
            && self.vol.popover.is_none_or(|r| !r.contains(p))
        {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    use nikgob_view::TrackId;

    use crate::screens::pages::{FILE, LibraryRowsOf, TrackAct as A, library_track_rows, widgets::{row_acts, row_items, sub_rows}};

    fn track(has_local: bool, liked: bool) -> TrackRow {
        TrackRow {
            id: TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local, liked, cover: None, clip: false, in_library: true,
        }
    }

    /// A track with its album and artist known.
    fn full(has_local: bool) -> TrackRow {
        TrackRow {
            album: Some("Album".into()),
            release_id: Some(nikgob_view::ReleaseId::new()),
            artists: nikgob_view::Credit::single("Artist", Some(nikgob_view::ArtistId::new())),
            ..track(has_local, false)
        }
    }

    /// The Now Playing and bar ⋮ are a library row's one list without
    /// «Сыграть следующим» / «В очередь» (the track is playing): the
    /// player's own rows before it, the sound's row and Now Playing's
    /// «Визуализатор» right before «Файл», Now Playing's delete last in it.
    #[test]
    fn the_player_menus_are_the_track_list_with_their_own_rows() {
        let ctx = egui::Context::default();
        let state = ViewState::default();
        let t = full(true);
        let of = LibraryRowsOf { marks: &state.dislikes, info: true, also: None, torrents: false, wished: None };
        let list: Vec<PlayerAct> = row_acts(&library_track_rows(&t, of))
            .into_iter()
            .filter(|a| !matches!(a, A::Next | A::Queue))
            .map(PlayerAct::Track)
            .collect();
        let less = |rows: &[Row<PlayerAct>], extra: &[PlayerAct]| -> Vec<PlayerAct> {
            row_acts(rows).into_iter().filter(|a| !extra.contains(a)).collect()
        };
        let (eq, viz) = (PlayerAct::Track(A::Equalizer), PlayerAct::Own(Act::Visualizer));
        let np = Chrome::player_rows(&ctx, &t, &state, &[Act::Visualizer], true, true);
        assert_eq!(less(&np, &[eq, viz, PlayerAct::Own(Act::Trash)]), list);
        let acts = row_acts(&np);
        let info = acts.iter().position(|a| *a == PlayerAct::Track(A::Info)).unwrap();
        assert_eq!(&acts[info - 1..info + 4], [PlayerAct::Track(A::Share), PlayerAct::Track(A::Info), eq, viz, PlayerAct::Track(A::Fetch)]);
        let file = row_acts(sub_rows(&np, FILE).unwrap());
        assert_eq!(file[0], PlayerAct::Track(A::Fetch));
        assert_eq!(file.last(), Some(&PlayerAct::Own(Act::Trash)));
        let top = [Act::Like, Act::Lyrics, Act::Volume];
        let bar = Chrome::player_rows(&ctx, &t, &state, &top, true, false);
        assert_eq!(&row_acts(&bar)[..3], top.map(PlayerAct::Own));
        assert_eq!(&less(&bar, &[eq])[3..], &list[..]);
        assert_eq!(&list[..6], [A::Radio, A::Playlist, A::Album, A::Artist, A::Share, A::Info].map(PlayerAct::Track));
        // Not in the library (no file, no like): nothing to delete.
        let stream = row_acts(&Chrome::player_rows(&ctx, &track(false, false), &state, &[], true, true));
        assert!(!stream.contains(&PlayerAct::Own(Act::Trash)));
    }

    /// The bar's ⋮ over the page of the playing track's album has no «К
    /// альбому» (nor «К исполнителю» over its artist's); with no page under
    /// it (Now Playing) both are there.
    #[test]
    fn the_player_menu_leaves_out_the_page_under_it() {
        use crate::screens::pages::{Here, set_here};
        let ctx = egui::Context::default();
        let state = ViewState::default();
        let t = full(true);
        let acts_on = |route: Option<Route>| {
            let mut acts = Vec::new();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if let Some(r) = &route {
                    set_here(ui.ctx(), Here::of(r));
                }
                acts = row_acts(&Chrome::player_rows(ui.ctx(), &t, &state, &[], true, false));
            });
            out.textures_delta.clear();
            acts
        };
        let album = PlayerAct::Track(A::Album);
        let artist = PlayerAct::Track(A::Artist);
        let on_album = acts_on(Some(Route::Release(t.release_id.unwrap())));
        assert!(!on_album.contains(&album) && on_album.contains(&artist));
        let on_artist = acts_on(Some(Route::Artist(*t.artists.main().unwrap())));
        assert!(on_artist.contains(&album) && !on_artist.contains(&artist));
        // Two frames with no page: Now Playing covers it.
        acts_on(None);
        let covered = acts_on(None);
        assert!(covered.contains(&album) && covered.contains(&artist));
    }

    /// The sound of the playing track is one row of its menu, never
    /// marked: the equalizer with its key, and with the speed of tracks on
    /// and a track playing the speed first, with both keys.
    #[test]
    fn the_player_menus_open_the_sound_sheet() {
        let ctx = egui::Context::default();
        let t = track(true, false);
        let mut state = ViewState::default();
        let sound = |state: &ViewState| -> Vec<(PlayerAct, String, Option<String>, bool)> {
            let rows = Chrome::player_rows(&ctx, &t, state, &[], true, false);
            row_items(&rows)
                .into_iter()
                .filter(|(a, _)| matches!(a, PlayerAct::Track(A::Speed | A::Equalizer)))
                .map(|(a, item)| (a, item.label.to_string(), item.shortcut.as_ref().map(ToString::to_string), item.selected))
                .collect()
        };
        let row = |act, keys: &str| (PlayerAct::Track(act), "Скорость и эквалайзер".to_owned(), Some(keys.to_owned()), false);
        assert_eq!(sound(&state), vec![row(A::Equalizer, "E")]);
        state.prefs.eq_on = true;
        state.speeds.enabled = true;
        // Nothing plays: no speed to set.
        assert_eq!(sound(&state), vec![row(A::Equalizer, "E")]);
        state.player.current = Some(t.clone());
        assert_eq!(sound(&state), vec![row(A::Speed, "S, E")]);
    }

    /// «Меню трека» places the player's own items too: the visualizer in
    /// «Файл», the sound's row hidden, «Изменить текст…» in the menu.
    #[test]
    fn the_settings_place_the_items_of_the_player_menus() {
        let ctx = egui::Context::default();
        use nikgob_view::{TrackMenuItem as I, TrackMenuPlace as P};
        let t = track(true, false);
        let mut state = ViewState::default();
        state.lyrics.track = Some(t.id);
        state.lyrics.source = Some(nikgob_view::LyricsSource::File);
        let plain = row_acts(&Chrome::player_rows(&ctx, &t, &state, &[Act::Like, Act::Visualizer], false, true));
        state.prefs.set_track_menu_place(I::Visualizer, P::File);
        state.prefs.set_track_menu_place(I::Sound, P::Hidden);
        state.prefs.set_track_menu_place(I::LyricsEdit, P::Menu);
        let rows = Chrome::player_rows(&ctx, &t, &state, &[Act::Like, Act::Visualizer], false, true);
        let acts = row_acts(&rows);
        assert_eq!(acts.len(), plain.len() - 1);
        assert_eq!(acts[0], PlayerAct::Own(Act::Like));
        assert!(!acts.contains(&PlayerAct::Track(A::Equalizer)));
        let file = row_acts(sub_rows(&rows, FILE).unwrap());
        assert_eq!(file[..2], [PlayerAct::Own(Act::Visualizer), PlayerAct::Track(A::Fetch)]);
        assert_eq!(file.last(), Some(&PlayerAct::Own(Act::Trash)));
        let at = rows.iter().position(|r| matches!(r, Row::Sub(..))).unwrap();
        assert!(matches!(rows[at - 1], Row::Item(PlayerAct::Own(Act::EditLyrics), _)));
    }

    /// «Изменить текст…» is in «Файл» of the playing track's menu, right
    /// after the tags, while the shown lyrics are its own and have a text.
    #[test]
    fn the_lyrics_editor_opens_from_the_player_menus() {
        let ctx = egui::Context::default();
        let t = track(true, false);
        let mut state = ViewState::default();
        let edit = PlayerAct::Own(Act::EditLyrics);
        let file = |state: &ViewState| {
            let rows = Chrome::player_rows(&ctx, &t, state, &[], false, false);
            row_acts(sub_rows(&rows, FILE).unwrap())
        };
        assert!(!file(&state).contains(&edit));
        state.lyrics.track = Some(t.id);
        state.lyrics.source = Some(nikgob_view::LyricsSource::File);
        assert_eq!(&file(&state)[..2], [PlayerAct::Track(A::Fetch), edit]);
        // The lyrics of another track: not this menu's to edit.
        state.lyrics.track = Some(TrackId::new());
        assert!(!file(&state).contains(&edit));
    }

    /// One song on two albums: the Now Playing and bar ⋮ offer the other
    /// album right after «К альбому» and «Разные песни» last, like the
    /// track menu of a list.
    #[test]
    fn the_other_album_of_a_song_is_in_the_player_menus() {
        let ctx = egui::Context::default();
        let t = full(true);
        let mut state = ViewState::default();
        let other = nikgob_view::ReleaseId::new();
        state.also_on = std::sync::Arc::new([(t.id, (other, "Live".to_owned()))].into_iter().collect());
        let rows = Chrome::player_rows(&ctx, &t, &state, &[], false, false);
        let acts = row_acts(&rows);
        assert_eq!(&acts[2..5], [A::Album, A::AlsoOn, A::Artist].map(PlayerAct::Track));
        assert_eq!(acts.last(), Some(&PlayerAct::Track(A::Apart)));
        let labels: Vec<String> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Item(_, item) => Some(item.label.to_string()),
                _ => None,
            })
            .collect();
        assert!(labels.iter().any(|l| l == "Есть и на альбоме «Live»"));
        assert!(labels.iter().any(|l| l == "Разные песни"));
    }
}
