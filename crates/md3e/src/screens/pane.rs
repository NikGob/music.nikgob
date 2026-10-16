//! The supporting pane: Очередь | Текст | Инфо (md3e-player.md §2.4).

use egui::{Rect, Ui};
use nikgob_view::{Command, TrackRow, ViewState};

use super::{
    PaneTab,
    chrome::{Chrome, replace_query},
    common,
};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, ConnectedGroup, IconButton, Leading, ListItem, LoadingIndicator, Size, Style,
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
    chrome: &mut Chrome,
    bass: f32,
    out: &mut Vec<Command>,
) -> PaneOutput {
    let tabs = ["Очередь", "Текст", "Инфо"];
    let idx = match tab {
        PaneTab::Queue => 0,
        PaneTab::Lyrics => 1,
        PaneTab::Info => 2,
    };
    ui.add_space(8.0);
    // Tabs on the left, ✕ on the right.
    let close_w = Size::S.height();
    let row = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), close_w));
    let w = ((row.width() - 4.0 - close_w - 8.0) / 3.0).floor();
    let mut gui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_size(row.min, egui::vec2(3.0 * w + 4.0, close_w)))
            .id_salt("pane-tabs-row"),
    );
    let mut closed = false;
    if let Some(i) = ConnectedGroup::new("pane-tabs", &tabs)
        .equal_width(w)
        .reselect(true)
        .show(&mut gui, idx)
    {
        if i == idx {
            closed = true;
        } else {
            *tab = [PaneTab::Queue, PaneTab::Lyrics, PaneTab::Info][i];
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
        PaneTab::Queue => {
            queue(ui, state, chrome, out);
            false
        }
        PaneTab::Lyrics => lyrics(ui, state, bass, out),
        PaneTab::Info => {
            info(ui, state, out);
            false
        }
    };
    PaneOutput { closed, configure_lyrics }
}

/// A queue row: cover, title and artist.
fn queue_item<'a>(t: &'a TrackRow, salt: impl egui::AsId) -> ListItem<'a> {
    let uri = t.cover.as_ref().map(|c| c.uri());
    ListItem::new(&t.title)
        .supporting(&t.artist)
        .leading(Leading::Image(uri, Icon::MusicNote))
        .id_salt(salt)
}

pub(super) fn queue(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let q = &state.queue;
    let bottom_h = 56.0;
    let list_h = ui.available_height() - bottom_h;
    egui::ScrollArea::vertical()
        .id_salt("queue-scroll")
        .max_height(list_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // What already played, dimmed, above the current track; a
            // double click goes back to it.
            if !q.prev.is_empty() {
                common::label(ui, "Ранее", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                ui.scope(|ui| {
                    ui.multiply_opacity(0.6);
                    for e in &q.prev {
                        let o = queue_item(&e.track, ("q-prev", e.entry)).muted(true).show(ui);
                        if o.response.double_clicked() {
                            out.push(Command::PlayQueueEntry(e.entry));
                        }
                    }
                });
                ui.add_space(12.0);
            }
            if let Some(t) = &q.current {
                let top = ui.cursor().min.y;
                common::label(ui, "Сейчас играет", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                let o = queue_item(t, "q-current").selected(true).show(ui);
                // The history must not push the current track out of view:
                // bring it to the top whenever it changes (and on first show).
                let key = ui.id().with("q-shown");
                let shown = (t.id, q.prev.len());
                if ui.ctx().data(|d| d.get_temp::<(nikgob_view::TrackId, usize)>(key)) != Some(shown) {
                    ui.ctx().data_mut(|d| d.insert_temp(key, shown));
                    let r = Rect::from_min_max(egui::pos2(o.rect.left(), top), o.rect.max);
                    ui.scroll_to_rect(r, Some(egui::Align::TOP));
                }
                ui.add_space(12.0);
            }
            if q.next.is_empty() {
                common::label(ui, "Дальше ничего нет", TypeRole::BodyMedium, s.on_surface_variant);
                return;
            }
            common::label(ui, "Далее", TypeRole::TitleSmall, s.on_surface_variant);
            ui.add_space(4.0);
            let n = q.next.len();
            let mut rects: Vec<Rect> = Vec::with_capacity(n);
            // (index, entry) of the row whose handle is being dragged / was released.
            let mut dragging = None;
            let mut dropped = None;
            for (i, e) in q.next.iter().enumerate() {
                let entry = e.entry;
                let o = queue_item(&e.track, ("q", entry))
                    .drag_handle(true)
                    .trailing(|ui| {
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
                    })
                    .show(ui);
                rects.push(o.rect);
                if o.response.double_clicked() {
                    out.push(Command::PlayQueueEntry(entry));
                }
                if let Some(h) = &o.handle {
                    if h.dragged() {
                        dragging = Some((i, entry));
                    }
                    if h.drag_stopped() {
                        dropped = Some((i, entry));
                    }
                }
            }
            // The row under the pointer is where the dragged one lands.
            let target_at = |y: f32| rects.iter().position(|r| y < r.bottom()).unwrap_or(n - 1);
            let pointer = ui.ctx().pointer_latest_pos();
            if let (Some((from, _)), Some(p)) = (dragging, pointer) {
                let to = target_at(p.y);
                if to != from {
                    let r = rects[to];
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
            if let (Some((from, entry)), Some(p)) = (dropped, pointer) {
                let to = target_at(p.y);
                if to != from {
                    out.push(Command::MoveInQueue { entry, to });
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

pub(super) fn lyrics(ui: &mut Ui, state: &ViewState, bass: f32, out: &mut Vec<Command>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let l = &state.lyrics;
    let configure = super::lyrics::source_picker(ui, state, out);
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

pub(super) fn info(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let i = &state.info;
    egui::ScrollArea::vertical().id_salt("info").show(ui, |ui| {
        if let Some(q) = &i.quality {
            ui.add(
                Button::new(q.clone())
                    .style(Style::Outlined)
                    .size(Size::Xs)
                    .id_salt("info-q"),
            );
            ui.add_space(8.0);
        }
        let row = |ui: &mut Ui, k: &str, v: &str| {
            ui.label(TypeRole::LabelMedium.style().rich(k, s.on_surface_variant));
            ui.label(TypeRole::BodyMedium.style().rich(v, s.on_surface));
            ui.add_space(6.0);
        };
        if let Some(src) = &i.source {
            row(ui, "Источник", src);
        }
        if let Some(p) = &i.path {
            row(ui, "Файл", &p.display().to_string());
        }
        for (k, v) in &i.tags {
            row(ui, k, v);
        }
        match &state.player.current {
            None => {
                common::label(ui, "Ничего не играет", TypeRole::BodyMedium, s.on_surface_variant);
            }
            Some(t) => {
                ui.add_space(12.0);
                if ui
                    .add(
                        Button::new("Редактировать теги…")
                            .icon(Icon::Edit)
                            .style(Style::Outlined)
                            .size(Size::S)
                            .id_salt("info-tags"),
                    )
                    .clicked()
                {
                    out.push(Command::OpenTagEditor(t.id));
                }
                ui.add_space(8.0);
                if ui
                    .add(
                        Button::new("Заменить на лучшее качество…")
                            .icon(Icon::CloudDownload)
                            .style(Style::Tonal)
                            .size(Size::S)
                            .id_salt("info-replace"),
                    )
                    .clicked()
                {
                    out.push(Command::SearchTorrents(replace_query(t)));
                }
                ui.add_space(8.0);
            }
        }
    });
}
