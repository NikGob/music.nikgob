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

use super::{
    PaneTab,
    pages::{TrackAct, widgets::Row},
    tag_editor::TagEditorUi,
    visualizer::Viz,
};
use crate::{
    Elevation, Icon, Theme, TypeRole,
    components::{
        Dialog, IconButton, Menu, MenuItem, Size, TextInputDialog, Radii, anim,
        dialog::TextInputResult, paint,
    },
    tokens::shape,
};

/// A row of the playing track's ⋮ that only the player has: what does not
/// fit in the bar's pill, the visualizer, the delete. The track's own
/// actions are [`TrackAct`] from the one list of every track menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Like,
    Lyrics,
    Volume,
    Visualizer,
    Trash,
    /// «Включить с YouTube» (the developer profile only).
    Youtube,
}

/// What a row of the playing track's ⋮ stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerAct {
    /// The player's own row, before (or the delete after) the track's.
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
    /// The track the «Скорость» picker is for, and its anchor.
    speed_picker: Option<(TrackId, Rect)>,
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
            Act::Visualizer => MenuItem::new("Визуализатор")
                .icon(Icon::GraphicEq)
                .shortcut("F12"),
            // Only what is in the library (a file, or a like) can leave it.
            Act::Trash => MenuItem::new("Удалить из библиотеки").icon(Icon::Delete).enabled(t.has_local || t.liked),
            Act::Youtube => MenuItem::new("Включить с YouTube")
                .icon(Icon::PlayArrow)
                .shortcut("Y"),
        }
    }

    /// The rows of the playing track's ⋮: the player's own rows `top`, then
    /// the one list of a library track ([`super::pages::library_track_rows`])
    /// without «Сыграть следующим» / «В конец очереди» (it is playing), then
    /// the delete when `trash` and the track is in the library.
    pub fn player_rows(t: &TrackRow, state: &ViewState, top: &[Act], info: bool, trash: bool) -> Vec<Row<PlayerAct>> {
        // The developer profile's tracks have no files: the real song first.
        let youtube = nikgob_view::dev::profile().then_some(Act::Youtube);
        let mut rows: Vec<Row<PlayerAct>> = youtube
            .iter()
            .chain(top)
            .map(|&a| Row::Item(PlayerAct::Own(a), Self::item(a, t, state)))
            .collect();
        rows.push(Row::Divider);
        let also = state.also_on.get(&t.id);
        let list = super::pages::library_track_rows(t, &state.dislikes, info, also);
        for r in super::pages::with_speed_row(list, &state.speeds, t.id) {
            rows.push(match r {
                Row::Item(TrackAct::Next | TrackAct::Queue, _) => continue,
                Row::Item(a, item) => Row::Item(PlayerAct::Track(a), item),
                Row::Divider => Row::Divider,
                Row::Label(l) => Row::Label(l),
            });
        }
        if trash && (t.has_local || t.liked) {
            rows.push(Row::Divider);
            rows.push(Row::Item(PlayerAct::Own(Act::Trash), Self::item(Act::Trash, t, state)));
        }
        rows
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
            match act {
                PlayerAct::Own(a) => self.apply(a, track, anchor.rect, wh, out),
                PlayerAct::Track(a) => self.apply_track(&anchor.ctx, a, track, state, anchor.rect, wh, out),
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
            Act::Lyrics => self.show(PaneTab::Lyrics, wh),
            Act::Volume => self.vol.pinned = Some(anchor),
            Act::Visualizer => self.visualizer = true,
            Act::Trash => self.trash = Some((t.id, t.title.clone())),
            Act::Youtube => out.push(Command::PlayFromYoutube),
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
            A::Playlist => self.picker = Some(Picker { tracks: vec![t.id], anchor }),
            A::Radio => out.push(super::pages::radio_by_track(t.id)),
            A::Album => self.nav = t.release_id.map(Route::Release),
            A::AlsoOn => self.nav = state.also_on.get(&t.id).map(|(r, _)| Route::Release(*r)),
            A::Apart => out.push(Command::RecordingApart(t.id)),
            // Several artists: which one.
            A::Artist => self.nav = super::artist_links::go_to(ctx, super::artist_links::library(&t.artists)),
            A::Info => self.show(PaneTab::Info, wh),
            A::Share => out.push(Command::Share(nikgob_view::ShareTarget::Track(t.id))),
            A::Speed => self.speed_picker = Some((t.id, anchor)),
            A::Fetch if t.has_local => out.push(Command::OpenTagEditor(t.id)),
            A::Fetch => out.push(Command::DownloadTracks(vec![t.id])),
            // The whole album the track is from, the files it lacks.
            A::AlbumDownload => out.extend(t.release_id.map(Command::DownloadRelease)),
            A::Torrents if t.has_local => out.push(Command::SearchTorrents(replace_query(t))),
            A::Torrents => out.push(Command::SearchTorrents(format!(
                "{} {}",
                t.artist,
                t.album.as_deref().unwrap_or(&t.title)
            ))),
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
        self.picker_ui(ctx, state, out);
        self.speed_picker_ui(ctx, state, out);
        self.dialogs(ctx, state, out);
        self.tag.show(ctx, state, out);
        self.volume_ui(ctx, state, out);
    }

    /// «Скорость» of the playing track: the five versions.
    fn speed_picker_ui(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let Some((track, anchor)) = self.speed_picker else { return };
        let mut open = true;
        let menu = super::pages::speed_menu("md3e-speed-picker", &state.speeds, track).align_right(true);
        let picked = menu.popup(ctx, anchor, &mut open);
        out.extend(picked.and_then(|i| super::pages::speed_command(track, i)));
        if picked.is_some() || !open {
            self.speed_picker = None;
        }
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
        // A dialog over Now Playing (the lyrics window, any other) scrolls
        // its own content: the wheel is its, not the volume's.
        if ctx.layer_id_at(p).is_some_and(|l| l.order >= egui::Order::Tooltip) && self.vol.popover.is_none_or(|r| !r.contains(p)) {
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

    use crate::screens::pages::{TrackAct as A, library_track_rows, widgets::row_acts};

    fn track(has_local: bool, liked: bool) -> TrackRow {
        TrackRow {
            id: TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local, liked, cover: None, clip: false,
        }
    }

    /// The Now Playing and bar ⋮ are a library row's one list without
    /// «Сыграть следующим» / «В конец очереди» (the track is playing): the
    /// player's own rows before it, Now Playing's delete after it.
    #[test]
    fn the_player_menus_are_the_track_list_with_their_own_rows() {
        let state = ViewState::default();
        let t = track(true, false);
        let list: Vec<PlayerAct> = row_acts(&library_track_rows(&t, &state.dislikes, true, None))
            .into_iter()
            .filter(|a| !matches!(a, A::Next | A::Queue))
            .map(PlayerAct::Track)
            .collect();
        let np = row_acts(&Chrome::player_rows(&t, &state, &[Act::Visualizer], true, true));
        assert_eq!(np[0], PlayerAct::Own(Act::Visualizer));
        assert_eq!(&np[1..np.len() - 1], &list[..]);
        assert_eq!(np.last(), Some(&PlayerAct::Own(Act::Trash)));
        let top = [Act::Like, Act::Lyrics, Act::Volume];
        let bar = row_acts(&Chrome::player_rows(&t, &state, &top, true, false));
        assert_eq!(&bar[..3], top.map(PlayerAct::Own));
        assert_eq!(&bar[3..], &list[..]);
        assert_eq!(&list[..6], [A::Playlist, A::Radio, A::Album, A::Artist, A::Info, A::Share].map(PlayerAct::Track));
        // Not in the library (no file, no like): nothing to delete.
        let stream = row_acts(&Chrome::player_rows(&track(false, false), &state, &[], true, true));
        assert!(!stream.contains(&PlayerAct::Own(Act::Trash)));
    }

    /// One song on two albums: the Now Playing and bar ⋮ offer the other
    /// album and «Разные песни» right after «К альбому», like the track
    /// menu of a list.
    #[test]
    fn the_other_album_of_a_song_is_in_the_player_menus() {
        let t = track(true, false);
        let mut state = ViewState::default();
        let other = nikgob_view::ReleaseId::new();
        state.also_on = std::sync::Arc::new([(t.id, (other, "Live".to_owned()))].into_iter().collect());
        let rows = Chrome::player_rows(&t, &state, &[], false, false);
        let acts = row_acts(&rows);
        assert_eq!(&acts[2..5], [A::Album, A::AlsoOn, A::Apart].map(PlayerAct::Track));
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
