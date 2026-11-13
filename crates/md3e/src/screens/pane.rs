//! The supporting pane: Текст | Инфо.

use egui::{Rect, Ui};
use nikgob_view::{Command, QueueEntry, Route, TrackId, TrackInfo, TrackRow, ViewState};

use super::{
    PaneTab,
    chrome::{Chrome, PlayerAct, Where},
    common,
    pages::{TrackAct, widgets::Row},
};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, ConnectedGroup, IconButton, Leading, ListItem, LoadingIndicator, Menu, MenuItem, Size, Style, Switch,
        TooltipExt,
    },
};

/// What the user asked of the pane this frame.
#[derive(Default)]
pub struct PaneOutput {
    /// Closed: the ✕ in the header or a click on the tab already open.
    pub closed: bool,
    /// The lyrics tab asked for its settings.
    pub configure_lyrics: bool,
}

pub fn show(
    ui: &mut Ui,
    state: &ViewState,
    tab: &mut PaneTab,
    bass: f32,
    out: &mut Vec<Command>,
) -> PaneOutput {
    // Only the parts turned on in Settings have a tab.
    let parts: Vec<PaneTab> = PaneTab::ALL.into_iter().filter(|t| t.on(&state.prefs)).collect();
    let tabs: Vec<&str> = parts
        .iter()
        .map(|t| match t {
            PaneTab::Lyrics => "Текст",
            PaneTab::Info => "Инфо",
        })
        .collect();
    let idx = parts.iter().position(|t| t == tab).unwrap_or(0);
    ui.add_space(8.0);
    // Tabs on the left, ✕ on the right.
    let close_w = Size::S.height();
    let row = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), close_w));
    let n = tabs.len().max(1) as f32;
    let gaps = 2.0 * (n - 1.0);
    let w = ((row.width() - gaps - close_w - 8.0) / n).floor();
    let mut gui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(row.min, egui::vec2(n * w + gaps, close_w)))
            .id_salt("pane-tabs-row"),
    );
    let mut closed = false;
    // No tabs while it closes after its last part was turned off.
    if !tabs.is_empty()
        && let Some(i) = ConnectedGroup::new("pane-tabs", &tabs)
            .equal_width(w)
            .reselect(true)
            .show(&mut gui, idx)
    {
        if i == idx {
            closed = true;
        } else {
            *tab = parts[i];
        }
    }
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(egui::pos2(row.right() - close_w, row.top()), egui::vec2(close_w, close_w)))
            .id_salt("pane-close-slot"),
    );
    if cui
        .add(IconButton::new(Icon::Close).size(Size::S).tooltip("Закрыть").id_salt("pane-close"))
        .clicked()
    {
        closed = true;
    }
    ui.advance_cursor_after_rect(row);
    ui.add_space(12.0);
    let configure_lyrics = match tab {
        PaneTab::Lyrics => lyrics(ui, state, bass, out),
        PaneTab::Info => {
            info(ui, state, out);
            false
        }
    };
    PaneOutput { closed, configure_lyrics }
}

/// A queue row: cover, title, the artists and the album, each a link to
/// its page as in a track row (the pages they open, by link); a guest of
/// «✦ С дополнением» has ✦ before its artist.
fn queue_item<'a>(t: &'a TrackRow, guest: bool, salt: impl egui::AsId) -> (ListItem<'a>, Vec<Route>) {
    let uri = t.cover.as_ref().map(|c| c.uri());
    let (line, links, routes) = queue_line(t, guest);
    let item = ListItem::new(&t.title)
        .supporting(line)
        .supporting_links(links)
        .leading(Leading::Image(uri, Icon::MusicNote))
        .id_salt(salt);
    (item, routes)
}

/// The second line of a queue row ([`queue_item`]) with its links.
fn queue_line(t: &TrackRow, guest: bool) -> (String, Vec<std::ops::Range<usize>>, Vec<Route>) {
    let mut line = t.artist.clone();
    let (mut links, mut routes) = super::artist_links::library_spans(&t.artists, &t.artist);
    if let Some(a) = &t.album {
        let at = super::artist_links::append_source(&mut line, a);
        if let Some(r) = t.release_id {
            links.push(at);
            routes.push(Route::Release(r));
        }
    }
    if guest {
        let mark = format!("{GUEST_MARK} ");
        line.insert_str(0, &mark);
        for l in &mut links {
            *l = l.start + mark.len()..l.end + mark.len();
        }
    }
    (line, links, routes)
}

/// The mark of a guest of «✦ С дополнением» in its queue row.
pub(super) const GUEST_MARK: &str = "✦";

/// What a queue row's menu does: the queue's own rows, or a track's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueAct {
    /// «Слушать»: the queue goes on from this row.
    Play,
    /// «Сыграть следующим»: the row moves right after the track playing.
    Next,
    /// «Убрать из очереди».
    Remove,
    Track(TrackAct),
}

/// The menu of a queue row (its ⋮ and a right click): «Слушать» and
/// «Сыграть следующим» (moved right after the track playing, unless it is
/// there already), then the track's one list as every list has it (less
/// its own «Сыграть следующим» / «В очередь»: it is in the queue, and of
/// the speed: it does not play), and «Убрать из очереди» last. The playing
/// row has the player's menu.
fn queue_menu_rows(ctx: &egui::Context, e: &QueueEntry, state: &ViewState) -> Vec<Row<QueueAct>> {
    use super::pages as p;
    let t = &e.track;
    let mut rows = vec![Row::Item(QueueAct::Play, MenuItem::new("Слушать").icon(Icon::PlayArrow))];
    let first = state.queue.next.first().map(|n| n.entry) == Some(e.entry);
    if state.queue.current_entry.is_some_and(|c| c != e.entry) && !first {
        rows.push(Row::Item(QueueAct::Next, MenuItem::new(p::PLAY_NEXT).icon(Icon::QueueMusic)));
    }
    rows.push(Row::Divider);
    let wish = p::widgets::WishIndex::of(ctx, &state.downloads.wishlist);
    let track = p::library_track_rows(t, p::LibraryRowsOf::of(state, t, &wish));
    // Nothing that leads back to the page beside the pane.
    let also = state.also_on.get(&t.id).map(|(r, _)| *r);
    let track = p::here(ctx).trim(track, t, also, Some);
    let track = p::placed(track, &state.prefs.track_menu, |a| a.menu_item(t.has_local));
    rows.extend(track.into_iter().filter(|r| !matches!(r, Row::Item(TrackAct::Next | TrackAct::Queue, _))).map(|r| r.map(&mut QueueAct::Track)));
    // With «Разные песни»: one last group.
    if !matches!(rows.last(), Some(Row::Item(QueueAct::Track(TrackAct::Apart), _))) {
        rows.push(Row::Divider);
    }
    rows.push(Row::Item(QueueAct::Remove, MenuItem::new("Убрать из очереди").icon(Icon::Close)));
    rows
}

#[allow(clippy::too_many_arguments)]
fn queue_apply(
    ctx: &egui::Context,
    act: QueueAct,
    e: &QueueEntry,
    state: &ViewState,
    chrome: &mut Chrome,
    anchor: Rect,
    out: &mut Vec<Command>,
) {
    match act {
        QueueAct::Play => out.push(Command::PlayQueueEntry(e.entry)),
        QueueAct::Next => {
            if let Some(cur) = state.queue.current_entry {
                out.push(Command::MoveInQueue { entry: e.entry, anchor: cur, after: true });
            }
        }
        QueueAct::Remove => out.push(Command::RemoveFromQueue(e.entry)),
        // «Источник и качество» of a row that is not playing: its own
        // window ([`Chrome::apply_track`]).
        QueueAct::Track(a) => chrome.apply_track(ctx, a, &e.track, state, anchor, Where::Bar, out),
    }
}

/// A movable queue row (a drag handle; ⋮ and ✕ under the pointer, so the
/// narrow pane keeps the titles whole); played ones are dimmed. Returns its
/// rect and whether its handle is dragged / was released.
fn queue_row(ui: &mut Ui, e: &QueueEntry, played: bool, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) -> (Rect, bool, bool) {
    let entry = e.entry;
    // Hovered last frame, or its ⋮ menu is open (the pointer is on the menu).
    let key = ui.id().with(("q-actions", entry));
    let show_actions = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    ui.scope(|ui| {
        if played {
            ui.multiply_opacity(0.6);
        }
        let mut more = None;
        let (item, links) = queue_item(&e.track, e.guest, ("q", entry));
        let o = item
            .muted(played)
            .drag_handle(true)
            .trailing(|ui| {
                if !show_actions {
                    return;
                }
                if ui
                    .add(
                        IconButton::new(Icon::Close)
                            .size(Size::Xs)
                            .tooltip("Убрать из очереди")
                            .id_salt(("q-x", entry)),
                    )
                    .clicked()
                {
                    out.push(Command::RemoveFromQueue(entry));
                }
                more = Some(ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("q-more", entry))));
            })
            .show(ui);
        if let Some(r) = o.link_clicked.and_then(|k| links.get(k).cloned()) {
            chrome.nav = Some(r);
        }
        if o.response.double_clicked() {
            out.push(Command::PlayQueueEntry(entry));
        }
        // Why a radio or a mix picked it: its first pick and new music.
        if let Some(why) = &e.why {
            o.response.clone().md3_tooltip(why, None);
        }
        let mut menu_open = false;
        let ctx = ui.ctx().clone();
        if let Some(m) = &more {
            let (menu, acts) = super::pages::widgets::menu_of(("q-menu", entry), queue_menu_rows(&ctx, e, state));
            if let Some(act) = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied()) {
                queue_apply(&ctx, act, e, state, chrome, m.rect, out);
            }
            menu_open = ui.ctx().data(|d| d.get_temp::<bool>(m.id.with("md3e-menu-open"))).unwrap_or(false);
        }
        if !Menu::context_idle(&o.response, ("q-ctx", entry)) {
            let (menu, acts) = super::pages::widgets::menu_of(("q-ctx", entry), queue_menu_rows(&ctx, e, state));
            if let Some(act) = menu.context_menu(&o.response).and_then(|i| acts.get(i).copied()) {
                let at = Rect::from_center_size(ctx.pointer_latest_pos().unwrap_or(o.rect.center()), egui::vec2(1.0, 1.0));
                queue_apply(&ctx, act, e, state, chrome, at, out);
            }
        }
        let now = o.response.contains_pointer() || menu_open;
        if now != show_actions {
            ui.ctx().data_mut(|d| d.insert_temp(key, now));
            ui.ctx().request_repaint();
        }
        let (drag, drop) = o.handle.as_ref().map_or((false, false), |h| (h.dragged(), h.drag_stopped()));
        (o.rect, drag, drop)
    })
    .inner
}

/// How tall a queue row is (two lines beside a cover).
const QUEUE_ROW_H: f32 = crate::components::list::TWO_LINE;

/// The rows of `entries`, one under another: those in sight (see
/// [`Shown`](super::pages::widgets::Shown)) and the one dragged (`held`)
/// are laid out by `row`, the others only take their room. Every row's
/// rect goes into `rows`, for the drag and drop.
fn queue_rows(
    ui: &mut Ui,
    key: &str,
    entries: &[QueueEntry],
    held: Option<u64>,
    rows: &mut Vec<(u64, Rect)>,
    mut row: impl FnMut(&mut Ui, &QueueEntry, &mut Vec<(u64, Rect)>),
) {
    use super::pages::widgets::{Shown, span_range};
    let step = QUEUE_ROW_H + ui.spacing().item_spacing.y;
    let clip = ui.clip_rect();
    let now = span_range(entries.len(), ui.cursor().top(), step, QUEUE_ROW_H, clip.top(), clip.bottom());
    let shown = Shown::of(ui.ctx(), ui.id().with(("q-rows", key)), now);
    for (i, e) in entries.iter().enumerate() {
        if shown.contains(i) || held == Some(e.entry) {
            row(ui, e, rows);
        } else {
            let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), QUEUE_ROW_H));
            rows.push((e.entry, rect));
        }
    }
}

pub(super) fn queue(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let q = &state.queue;
    // A radio says so above the list, and where it picks from; autoplay
    // can be switched off right there.
    if let Some(r) = &q.radio {
        match &r.wave {
            Some(w) => wave_header(ui, r, w, state, chrome, out),
            None => {
                if radio_header(ui, r, state, chrome, out) {
                    out.push(Command::SetPref(nikgob_view::PrefPatch::Autoplay(false)));
                }
            }
        }
        ui.add_space(8.0);
    }
    let bottom_h = 56.0;
    let list_h = ui.available_height() - bottom_h;
    egui::ScrollArea::vertical()
        .id_salt("queue-scroll")
        .max_height(list_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // The queue is a temporary playlist: every row but the current
            // one can be removed or dragged anywhere, across the current
            // track too (a played one back up next, an upcoming one into
            // the history). Rows in queue order: (entry, rect).
            let mut rows: Vec<(u64, Rect)> = Vec::new();
            // Index into `rows` of the row whose handle is dragged / was released.
            let mut dragging = None;
            let mut dropped = None;
            let mut current_pick = None;
            let mut current_link = None;
            // The row dragged last frame stays laid out wherever it is.
            let drag_key = ui.id().with("q-dragged");
            let held = ui.ctx().data(|d| d.get_temp::<u64>(drag_key));
            let mut row = |ui: &mut Ui, e: &QueueEntry, played: bool, rows: &mut Vec<(u64, Rect)>| {
                let (rect, drag, drop) = queue_row(ui, e, played, state, chrome, out);
                debug_assert_eq!(rect.height(), QUEUE_ROW_H, "queue rows are of one height");
                if drag {
                    dragging = Some(rows.len());
                }
                if drop {
                    dropped = Some(rows.len());
                }
                rows.push((e.entry, rect));
            };
            // What already played, dimmed, above the current track; a
            // double click goes back to it.
            if !q.prev.is_empty() {
                common::label(ui, "Ранее", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                queue_rows(ui, "prev", &q.prev, held, &mut rows, |ui, e, rows| row(ui, e, true, rows));
                ui.add_space(12.0);
            }
            if let Some(t) = &q.current {
                let top = ui.cursor().min.y;
                common::label(ui, "Сейчас играет", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                let (item, links) = queue_item(t, q.current_guest, "q-current");
                let o = item.selected(true).show(ui);
                current_link = o.link_clicked.and_then(|k| links.get(k).cloned());
                if let Some(why) = &q.current_why {
                    o.response.clone().md3_tooltip(why, None);
                }
                // The player's menu (the same as the bar's ⋮). Done after
                // the rows: they hold `chrome` and `out` meanwhile.
                if !Menu::context_idle(&o.response, "q-ctx-current") {
                    let rows = Chrome::player_rows(ui.ctx(), t, state, &[], true, false);
                    let (menu, acts) = super::pages::widgets::menu_of("q-ctx-current", rows);
                    current_pick = menu.context_menu(&o.response).and_then(|i| acts.get(i).copied()).map(|a: PlayerAct| (a, t));
                }
                if let Some(entry) = q.current_entry {
                    rows.push((entry, o.rect));
                }
                // The history must not push the current track out of view:
                // bring it to the top whenever it changes (and on first show),
                // not when a row is removed or moved.
                let key = ui.id().with("q-shown");
                let shown = q.current_entry;
                if ui.ctx().data(|d| d.get_temp::<Option<u64>>(key)) != Some(shown) {
                    ui.ctx().data_mut(|d| d.insert_temp(key, shown));
                    let r = Rect::from_min_max(egui::pos2(o.rect.left(), top), o.rect.max);
                    ui.scroll_to_rect(r, Some(egui::Align::TOP));
                }
                ui.add_space(12.0);
            }
            let extending = q.radio.as_ref().is_some_and(|r| r.extending);
            if q.next.is_empty() && !extending {
                common::label(ui, "Дальше ничего нет", TypeRole::BodyMedium, s.on_surface_variant);
            } else if !q.next.is_empty() {
                common::label(ui, "Далее", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                queue_rows(ui, "next", &q.next, held, &mut rows, |ui, e, rows| row(ui, e, false, rows));
            }
            // The radio is looking for the next tracks: said where they
            // will appear.
            if extending {
                extending_row(ui);
            }
            // What comes after a queue that ends: autoplay.
            if q.radio.is_none() {
                autoplay_block(ui, state, chrome, out);
            }
            if let Some(r) = current_link {
                chrome.nav = Some(r);
            }
            if let Some((act, t)) = current_pick {
                let at = Rect::from_center_size(ui.ctx().pointer_latest_pos().unwrap_or_default(), egui::vec2(1.0, 1.0));
                chrome.pick(&ui.ctx().clone(), act, t, state, at, Where::Bar, out);
            }
            if rows.is_empty() {
                return;
            }
            // The row under the pointer is where the dragged one lands:
            // after it when dragged down, before it when dragged up.
            let last = rows.len() - 1;
            let target_at = |y: f32| rows.iter().position(|(_, r)| y < r.bottom()).unwrap_or(last);
            let pointer = ui.ctx().pointer_latest_pos();
            if let (Some(from), Some(p)) = (dragging, pointer) {
                let to = target_at(p.y);
                if to != from {
                    let r = rows[to].1;
                    let y = if to < from { r.top() } else { r.bottom() };
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            egui::pos2(r.left() + 16.0, y - 1.5),
                            egui::pos2(r.right() - 16.0, y + 1.5),
                        ),
                        2,
                        s.primary,
                    );
                }
            }
            match dragging {
                Some(from) => {
                    ui.ctx().data_mut(|d| d.insert_temp(drag_key, rows[from].0));
                }
                None => ui.ctx().data_mut(|d| d.remove::<u64>(drag_key)),
            }
            if let (Some(from), Some(p)) = (dropped, pointer) {
                let to = target_at(p.y);
                if to != from {
                    out.push(Command::MoveInQueue { entry: rows[from].0, anchor: rows[to].0, after: to > from });
                }
            }
        });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add(Button::new("Очистить").style(Style::Tonal).id_salt("q-clear"))
            .clicked()
        {
            out.push(Command::ClearQueue);
        }
        if ui
            .add(
                Button::new("Сохранить как плейлист")
                    .style(Style::Tonal)
                    .id_salt("q-save"),
            )
            .clicked()
        {
            chrome.ask_save_queue(state.player.context.as_deref().unwrap_or("Очередь"));
        }
    });
}

/// The end of a queue that is no radio: «Автовоспроизведение» with its
/// switch, and while it is on, where its tracks will come from.
fn autoplay_block(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    let p = &state.prefs;
    ui.add_space(12.0);
    let head = Header {
        title: "Автовоспроизведение",
        hover: if p.autoplay { "Когда очередь кончится, она продолжится похожим" } else { "Очередь закончится на последнем треке" },
        switch: Some(p.autoplay),
        source: p.autoplay.then_some(nikgob_view::RadioKind::Autoplay),
        salt: "q-autoplay",
        edge: false,
    };
    if let Some(on) = header(ui, head, state, chrome, out)
        && on != p.autoplay
    {
        out.push(Command::SetPref(nikgob_view::PrefPatch::Autoplay(on)));
    }
    ui.add_space(8.0);
}

/// A radio's (or autoplay's) header line, laid out like a row of the
/// queue so it reads as part of the list.
struct Header<'a> {
    title: &'a str,
    /// What the line means, on hover of the title.
    hover: &'a str,
    /// Autoplay's switch, and whether it is on.
    switch: Option<bool>,
    /// Where the tracks come from, as a menu under the title.
    source: Option<nikgob_view::RadioKind>,
    salt: &'a str,
    /// The icon at the content's edge and the title next to it (a header
    /// with chips under it, starting there), not in the rows' columns.
    edge: bool,
}

/// How tall a header is with a source line under its title, and without.
const HEADER_H: f32 = 48.0;
const HEADER_ONE_LINE_H: f32 = 40.0;

/// Draws `h`: the radio icon in the covers' column, the title in the
/// titles' column, under it where the tracks come from («Похожее из твоей
/// библиотеки ⌄», a menu of the choices bound to the same prefs as
/// Settings, and the way to autoplay's settings), the switch at the end.
/// Returns the switch's state after this frame.
fn header(ui: &mut Ui, h: Header<'_>, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) -> Option<bool> {
    use super::pages::{SettingId, settings_help};
    let s = Theme::get(ui.ctx()).scheme;
    let height = if h.source.is_some() { HEADER_H } else { HEADER_ONE_LINE_H };
    let row = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), height));
    let cover_x = if h.edge { row.left() + 16.0 + 12.0 } else { row.left() + 16.0 + 28.0 };
    let icon = Rect::from_center_size(egui::pos2(cover_x, row.center().y), egui::vec2(24.0, 24.0));
    Icon::Radio.widget(false, s.on_surface_variant, 24.0).paint_at(ui, icon);
    // The rows' title column: side 16, cover 56, gap 12 (ListItem); at the
    // edge, 16 past the icon.
    let text_x = if h.edge { icon.right() + 16.0 } else { row.left() + 16.0 + 56.0 + 12.0 };
    let mut right = row.right() - 16.0;
    let mut switched = h.switch;
    if let Some(on) = &mut switched {
        let slot = Rect::from_min_max(egui::pos2(right - 52.0, row.top()), egui::pos2(right, row.bottom()));
        ui.put(slot, Switch::new(on).id_salt((h.salt, "switch")))
            .md3_tooltip("Автовоспроизведение", None);
        right = slot.left() - 12.0;
    }
    let w = (right - text_x).max(1.0);
    let title = {
        let mut job = egui::text::LayoutJob::single_section(h.title.to_owned(), TypeRole::TitleSmall.style().text_format(s.on_surface));
        job.wrap = egui::text::TextWrapping::truncate_at_width(w);
        crate::typography::layout(ui.painter(), job)
    };
    let p = &state.prefs;
    let chevron = 16.0;
    let sub = h.source.map(|kind| {
        let mut job = egui::text::LayoutJob::single_section(
            settings_help::radio_source_line_of(p, kind).to_owned(),
            TypeRole::BodySmall.style().text_format(s.on_surface_variant),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((w - chevron).max(1.0));
        (kind, crate::typography::layout(ui.painter(), job))
    });
    let sub_h = sub.as_ref().map_or(0.0, |(_, g)| g.size().y);
    let top = row.center().y - (title.size().y + sub_h) * 0.5;
    let title_rect = Rect::from_min_size(egui::pos2(text_x, top), title.size());
    ui.painter().galley(title_rect.min, title, s.on_surface);
    ui.interact(title_rect, ui.id().with((h.salt, "title")), egui::Sense::hover())
        .md3_tooltip(h.hover, None);
    if let Some((kind, galley)) = sub {
        let at = egui::pos2(text_x, title_rect.bottom());
        let hit = Rect::from_min_size(at, egui::vec2(galley.size().x + chevron, galley.size().y)).expand2(egui::vec2(4.0, 2.0));
        let resp = ui
            .interact(hit, ui.id().with((h.salt, "source")), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let color = if resp.hovered() { s.on_surface } else { s.on_surface_variant };
        let chevron_rect = Rect::from_min_size(
            egui::pos2(at.x + galley.size().x + 2.0, at.y + galley.size().y * 0.5 - 7.0),
            egui::vec2(14.0, 14.0),
        );
        ui.painter().galley(at, galley, color);
        Icon::ExpandMore.widget(false, color, 14.0).paint_at(ui, chevron_rect);
        let chosen = settings_help::radio_source_of(p, kind);
        if let Some(pick) = source_menu(h.salt, chosen).attach(&resp) {
            match source_pick(pick, chosen) {
                SourcePick::Choice(i) => {
                    out.extend(settings_help::radio_source_patches(p, kind, i).into_iter().map(Command::SetPref));
                }
                SourcePick::Settings => chrome.setting = Some(SettingId::Autoplay),
                SourcePick::Same => {}
            }
        }
    }
    ui.advance_cursor_after_rect(row);
    switched
}

/// The source menu of a header: «Скачанное · Библиотека · С новым» (the
/// one in use checked), then autoplay's settings.
fn source_menu(salt: &str, chosen: usize) -> Menu {
    let mut menu = Menu::new((salt, "source-menu"));
    for (i, c) in super::pages::SettingId::RadioSource.choices().iter().enumerate() {
        menu = menu.item(MenuItem::new(c.label).selected(i == chosen));
    }
    menu.divider().item(MenuItem::new("Настроить автовоспроизведение…").icon(Icon::Settings))
}

#[derive(Debug, PartialEq, Eq)]
enum SourcePick {
    Choice(usize),
    Same,
    Settings,
}

/// What item `pick` of [`source_menu`] asks for.
fn source_pick(pick: usize, chosen: usize) -> SourcePick {
    let n = super::pages::SettingId::RadioSource.choices().len();
    match pick {
        i if i == chosen => SourcePick::Same,
        i if i < n => SourcePick::Choice(i),
        _ => SourcePick::Settings,
    }
}

/// What a radio queue is called above its list: «Радио · Кино» as the
/// runtime names it; an endless mix («Спокойное», «Новое для тебя») says
/// it is a mix.
pub(super) fn radio_title(title: &str) -> String {
    if title.starts_with("Радио") {
        title.to_owned()
    } else {
        format!("Микс · {title}")
    }
}

/// The radio line above the queue: its title (autoplay names what it
/// continues), where it picks from, and autoplay's switch; returns whether
/// it was switched off.
fn radio_header(
    ui: &mut Ui,
    r: &nikgob_view::RadioView,
    state: &ViewState,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) -> bool {
    let (title, hover) = if r.autoplay {
        (r.title.clone(), "Автовоспроизведение: дописывается, пока играет")
    } else {
        (radio_title(&r.title), "Дописывается сама, пока играет")
    };
    let head = Header {
        title: &title,
        hover,
        switch: r.autoplay.then_some(true),
        source: Some(r.kind),
        salt: "q-radio",
        edge: false,
    };
    header(ui, head, state, chrome, out) == Some(false)
}

/// «Моя волна» above the queue: its title, and under it its filters as
/// drop-down chips (novelty, character, the user's scene); a chip changed
/// rebuilds the tail, the track playing goes on.
fn wave_header(ui: &mut Ui, r: &nikgob_view::RadioView, w: &nikgob_view::WaveView, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    use nikgob_view::{WaveCharacter, WaveFilter, WaveNovelty, WaveScene};
    let head = Header { title: &r.title, hover: "Подстраивается под тебя, пока играет", switch: None, source: None, salt: "q-wave", edge: true };
    header(ui, head, state, chrome, out);
    let f = w.filter;
    let mut picked: Option<WaveFilter> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.add_space(16.0);
        // Each chip always holds a value (the default is one too): all are
        // drawn the same, as a selected drop-down chip showing it.
        let chip = |ui: &mut Ui, salt: &str, label: &str, options: Vec<(&str, bool)>| -> Option<usize> {
            let resp = ui.add(crate::components::FilterChip::new(label, true).one_of().trailing_icon(Icon::ExpandMore).id_salt(salt));
            let mut menu = Menu::new((salt, "menu"));
            for (label, selected) in options {
                menu = menu.item(MenuItem::new(label).selected(selected));
            }
            menu.attach(&resp)
        };
        let novelty: Vec<(&str, bool)> = WaveNovelty::ALL.iter().map(|n| (n.title(), *n == f.novelty)).collect();
        if let Some(i) = chip(ui, "q-wave-novelty", f.novelty.title(), novelty) {
            picked = Some(WaveFilter { novelty: WaveNovelty::ALL[i], ..f });
        }
        let character: Vec<(&str, bool)> = WaveCharacter::ALL.iter().map(|c| (c.title(), *c == f.character)).collect();
        if let Some(i) = chip(ui, "q-wave-character", f.character.title(), character) {
            picked = Some(WaveFilter { character: WaveCharacter::ALL[i], ..f });
        }
        let scene_title = w.scenes.iter().find(|c| c.scene == f.scene).map_or("Все", |c| c.title.as_str());
        let mut scenes: Vec<(&str, bool)> = vec![("Все", f.scene == WaveScene::All)];
        scenes.extend(w.scenes.iter().map(|c| (c.title.as_str(), c.scene == f.scene)));
        if let Some(i) = chip(ui, "q-wave-scene", scene_title, scenes) {
            let scene = if i == 0 { WaveScene::All } else { w.scenes.get(i - 1).map_or(WaveScene::All, |c| c.scene) };
            picked = Some(WaveFilter { scene, ..f });
        }
    });
    if let Some(p) = picked.filter(|p| *p != f) {
        out.push(Command::SetWave(p));
    }
}

/// «Подбираю ещё…» at the end of a radio queue while it extends, lined up
/// with the covers of the rows.
fn extending_row(ui: &mut Ui) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(4.0);
    let row = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 56.0));
    let slot = Rect::from_center_size(egui::pos2(row.left() + 16.0 + 28.0, row.center().y), egui::vec2(32.0, 32.0));
    ui.put(slot, LoadingIndicator::new().size(32.0));
    ui.painter().text(
        egui::pos2(row.left() + 16.0 + 56.0 + 12.0, row.center().y),
        egui::Align2::LEFT_CENTER,
        "Подбираю ещё…",
        TypeRole::BodyMedium.font_id(),
        s.on_surface_variant,
    );
    ui.advance_cursor_after_rect(row);
}

pub(super) fn lyrics(ui: &mut Ui, state: &ViewState, bass: f32, out: &mut Vec<Command>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let l = &state.lyrics;
    let configure = super::lyrics_ui::control(ui, state, None, out);
    ui.add_space(4.0);
    if l.loading {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.add(LoadingIndicator::new());
            ui.add_space(8.0);
            common::label(ui, "Ищу текст…", TypeRole::BodyMedium, s.on_surface_variant);
        });
        return configure;
    }
    if l.lines.is_empty() {
        let text = l.plain.as_deref().unwrap_or("Текста нет");
        egui::ScrollArea::vertical().id_salt("lyrics-plain").show(ui, |ui| {
            ui.label(TypeRole::BodyLarge.style().rich_para(text, s.on_surface_variant));
        });
        return configure;
    }
    super::lyrics::show(ui, state, bass, false, out) || configure
}

/// The track's info as Now Playing shows it: its headline with a close
/// button, then [`info_body`] under it (two columns on a wide panel). True
/// when the button was pressed.
pub(super) fn info_sheet(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let close_w = Size::S.height();
    let row = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), close_w));
    let mut tui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(row.min, egui::pos2(row.right() - close_w - 8.0, row.bottom())))
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("info-sheet-title"),
    );
    common::label(&mut tui, "Источник и качество", TypeRole::TitleMedium, s.on_surface);
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(egui::pos2(row.right() - close_w, row.top()), egui::vec2(close_w, close_w)))
            .id_salt("info-sheet-close-slot"),
    );
    let closed = cui
        .add(IconButton::new(Icon::Close).size(Size::S).tooltip("Закрыть (Esc)").id_salt("info-sheet-close"))
        .clicked();
    ui.advance_cursor_after_rect(row);
    ui.add_space(12.0);
    info_scroll(ui, "info-sheet", state.info.track, |ui| {
        info_body(ui, &state.info, state.player.current.as_ref(), out);
    });
    closed
}

/// The playing track's info in the side pane: [`info_body`].
pub(super) fn info(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    if state.player.current.is_none() {
        let s = Theme::get(ui.ctx()).scheme;
        common::label(ui, "Ничего не играет", TypeRole::BodyMedium, s.on_surface_variant);
        return;
    }
    info_scroll(ui, "info", state.info.track, |ui| {
        info_body(ui, &state.info, state.player.current.as_ref(), out);
    });
}

/// From this width on the info stands in two columns: the file and its
/// tags | the quality and its check.
pub(super) const INFO_TWO_COLUMNS: f32 = 520.0;
/// Between the two columns.
const INFO_COLUMN_GAP: f32 = 32.0;
/// Above the quality when it stands under the rest.
const INFO_QUALITY_GAP: f32 = 20.0;

/// A scroll area for the info that is at its top whenever it shows another
/// track or comes up again; it scrolls only what does not fit. Returns how
/// tall its content is.
pub(super) fn info_scroll(ui: &mut Ui, salt: &str, track: Option<TrackId>, add: impl FnOnce(&mut Ui)) -> f32 {
    let key = ui.id().with((salt, "info-shown"));
    let pass = ui.ctx().cumulative_pass_nr();
    let seen = ui.ctx().data(|d| d.get_temp::<(u64, Option<TrackId>)>(key));
    let fresh = seen.is_none_or(|(at, shown)| at + 1 < pass || shown != track);
    ui.ctx().data_mut(|d| d.insert_temp(key, (pass, track)));
    let mut area = egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, true]);
    if fresh {
        area = area.vertical_scroll_offset(0.0);
    }
    area.show(ui, add).content_size.y
}

/// A track's info `i`, in the pane, Now Playing's sheet and the window of
/// any track alike: where it comes from, its file, its tags and what can be
/// done with them ([`about`]), then the quality and its check. `track`:
/// its row, when there is one (a service's song has none).
pub(super) fn info_body(ui: &mut Ui, i: &TrackInfo, track: Option<&TrackRow>, out: &mut Vec<Command>) {
    let width = ui.available_width();
    let Some(d) = i.sound.as_ref() else {
        about(ui, i, track, out);
        return;
    };
    if width < INFO_TWO_COLUMNS {
        about(ui, i, track, out);
        ui.add_space(INFO_QUALITY_GAP);
        super::sound_quality::section(ui, d, i.track, out);
        ui.add_space(8.0);
        return;
    }
    // Side by side, both from the same top.
    let col = ((width - INFO_COLUMN_GAP) * 0.5).floor();
    let top = ui.cursor().min;
    let column = |ui: &mut Ui, left: f32, salt: &str| {
        ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(egui::pos2(left, top.y), egui::vec2(col, f32::INFINITY)))
                .layout(egui::Layout::top_down(egui::Align::Min))
                .id_salt(("info-column", salt)),
        )
    };
    let mut a = column(ui, top.x, "about");
    about(&mut a, i, track, out);
    let mut q = column(ui, top.x + col + INFO_COLUMN_GAP, "quality");
    super::sound_quality::section(&mut q, d, i.track, out);
    let bottom = a.min_rect().bottom().max(q.min_rect().bottom());
    ui.advance_cursor_after_rect(Rect::from_min_max(top, egui::pos2(top.x + width, bottom)));
    ui.add_space(8.0);
}

/// Where a track comes from, its file (with the way to it), its tags, and
/// what can be done: a file's tags edited, a stream downloaded, a clip's
/// song found.
fn about(ui: &mut Ui, i: &TrackInfo, track: Option<&TrackRow>, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let row = |ui: &mut Ui, k: &str, v: &str| {
        ui.label(TypeRole::LabelMedium.style().rich(k, s.on_surface_variant));
        ui.label(TypeRole::BodyMedium.style().rich(v, s.on_surface));
        ui.add_space(6.0);
    };
    if let Some(src) = &i.source {
        row(ui, "Источник", src);
    }
    // A line like the others: there is nothing to press.
    if let Some(q) = &i.quality {
        row(ui, "Качество", q);
    }
    if let Some(p) = &i.path {
        ui.label(TypeRole::LabelMedium.style().rich("Файл", s.on_surface_variant));
        ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(p.display().to_string(), s.on_surface)).selectable(true));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(Button::new("Показать в папке").icon(Icon::Folder).style(Style::Tonal).size(Size::Xs).id_salt("info-reveal"))
                .clicked()
            {
                reveal(p);
            }
            if ui
                .add(Button::new("Скопировать путь").style(Style::Text).size(Size::Xs).id_salt("info-copy-path"))
                .clicked()
            {
                ui.ctx().copy_text(p.display().to_string());
            }
        });
        ui.add_space(10.0);
    }
    for (k, v) in &i.tags {
        row(ui, k, v);
    }
    let Some(id) = track.map(|t| t.id).or(i.track) else { return };
    // A file has tags to edit; a stream can be downloaded.
    let file = track.map_or(i.path.is_some(), |t| t.has_local);
    // A clip: its song by hand; a stream from YouTube: another version.
    let find = track.and_then(super::find_song::label);
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        let (label, icon) = if file { ("Редактировать теги…", Icon::Edit) } else { ("Скачать", Icon::Download) };
        if ui
            .add(Button::new(label).icon(icon).style(Style::Outlined).size(Size::S).id_salt("info-tags"))
            .clicked()
        {
            out.push(if file { Command::OpenTagEditor(id) } else { Command::DownloadTracks(vec![id]) });
        }
        if let Some(find) = find
            && ui
                .add(Button::new(find).icon(Icon::MusicNote).style(Style::Outlined).size(Size::S).id_salt("info-find-song"))
                .clicked()
        {
            out.push(Command::FindSong { track: id, query: None });
        }
    });
}

/// Opens the file's folder with the file selected.
fn reveal(path: &std::path::Path) {
    // A test must never launch a program on the user's desktop.
    if cfg!(test) {
        panic!("a test tried to launch an external program");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // `explorer /select,"C:\a b\c.opus"`: the quotes must reach it as is.
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    // The file manager's own "show this file" (FileManager1, which Nautilus,
    // Dolphin, Nemo and others serve); without one, the folder opens.
    #[cfg(unix)]
    {
        let (path, uri) = (path.to_owned(), file_uri(path));
        let _ = std::thread::Builder::new().name("reveal".into()).spawn(move || {
            let shown = std::process::Command::new("dbus-send")
                .args([
                    "--session",
                    "--print-reply",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                ])
                .arg(format!("array:string:{uri}"))
                .arg("string:")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !shown && let Some(dir) = path.parent() {
                // This thread is the background: wait, so no zombie is left.
                let _ = std::process::Command::new("xdg-open").arg(dir).status();
            }
        });
    }
}

/// A `file://` URI for an absolute path, every byte outside the unreserved
/// set and `/` percent-encoded (a comma would split dbus-send's array).
#[cfg(unix)]
fn file_uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queue row says the album after the artists, a link to its page
    /// when the library has it; a guest's ✦ before them moves no link off
    /// its name.
    #[test]
    fn a_queue_row_links_its_album() {
        let r = nikgob_view::ReleaseId::new();
        let t = TrackRow {
            id: nikgob_view::TrackId::new(),
            title: "Звезда".into(),
            artist: "Кино".into(),
            album: Some("Группа крови".into()),
            release_id: Some(r),
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
        };
        for guest in [false, true] {
            let (line, links, routes) = queue_line(&t, guest);
            assert_eq!(&line[links.last().unwrap().clone()], "Группа крови", "{line}");
            assert_eq!(routes.last(), Some(&Route::Release(r)));
            assert_eq!(line.starts_with(GUEST_MARK), guest);
        }
    }

    #[cfg(unix)]
    #[test]
    fn revealed_files_are_uris() {
        assert_eq!(file_uri(std::path::Path::new("/home/u/Music/a b,c.opus")), "file:///home/u/Music/a%20b%2Cc.opus");
        assert_eq!(file_uri(std::path::Path::new("/м/1.flac")), "file:///%D0%BC/1.flac");
    }

    /// A queue row's menu is a track's one list with the queue's own rows:
    /// «Слушать», «Сыграть следующим» (moving it right after the track
    /// playing, unless it is there), «Убрать из очереди» last; no second
    /// «В очередь».
    #[test]
    fn radio_queue_is_named_and_its_menu_is_the_track_menu() {
        assert_eq!(radio_title("Радио · Кино"), "Радио · Кино");
        assert_eq!(radio_title("Спокойное"), "Микс · Спокойное");
        let song = |title: &str| TrackRow {
            id: nikgob_view::TrackId::new(),
            title: title.into(),
            artist: "Кино".into(),
            album: None,
            release_id: None,
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
        };
        let mut state = ViewState::default();
        state.queue.current_entry = Some(1);
        let first = QueueEntry { entry: 2, track: song("Звезда"), why: None, guest: false };
        let later = QueueEntry { entry: 3, track: song("Группа крови"), why: None, guest: false };
        state.queue.next = vec![first.clone(), later.clone()];
        let ctx = egui::Context::default();
        let acts = crate::screens::pages::widgets::row_acts(&queue_menu_rows(&ctx, &later, &state));
        assert_eq!(&acts[..3], [QueueAct::Play, QueueAct::Next, QueueAct::Track(TrackAct::Radio)]);
        assert_eq!(acts.last(), Some(&QueueAct::Remove));
        assert!(!acts.iter().any(|a| matches!(a, QueueAct::Track(TrackAct::Next | TrackAct::Queue))));
        assert!(acts.contains(&QueueAct::Track(TrackAct::Playlist)) && acts.contains(&QueueAct::Track(TrackAct::Share)));
        // Already next: nothing to move.
        let acts = crate::screens::pages::widgets::row_acts(&queue_menu_rows(&ctx, &first, &state));
        assert!(!acts.contains(&QueueAct::Next));
        // A row that does not play: no speed of its own, but its info.
        state.speeds.enabled = true;
        let acts = crate::screens::pages::widgets::row_acts(&queue_menu_rows(&ctx, &later, &state));
        assert!(!acts.contains(&QueueAct::Track(TrackAct::Speed)));
        assert!(acts.contains(&QueueAct::Track(TrackAct::Info)));

        let mut chrome = Chrome::default();
        let mut out = Vec::new();
        for act in [QueueAct::Play, QueueAct::Next, QueueAct::Remove, QueueAct::Track(TrackAct::Radio)] {
            queue_apply(&ctx, act, &later, &state, &mut chrome, Rect::NOTHING, &mut out);
        }
        assert_eq!(
            out,
            vec![
                Command::PlayQueueEntry(3),
                Command::MoveInQueue { entry: 3, anchor: 1, after: true },
                Command::RemoveFromQueue(3),
                Command::StartRadio { seed: nikgob_view::RadioSeed::Track(later.track.id) },
            ]
        );
    }

    #[test]
    fn the_source_menu_picks_a_choice_or_opens_the_settings() {
        assert_eq!(crate::screens::pages::SettingId::RadioSource.choices().len(), 3);
        assert_eq!(source_pick(0, 1), SourcePick::Choice(0));
        assert_eq!(source_pick(1, 1), SourcePick::Same);
        assert_eq!(source_pick(2, 1), SourcePick::Choice(2));
        // The divider is no item: the settings come right after the choices.
        assert_eq!(source_pick(3, 1), SourcePick::Settings);
    }
}
