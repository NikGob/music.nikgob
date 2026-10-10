//! The supporting pane: Очередь | Текст | Инфо (md3e-player.md §2.4).

use egui::{Rect, Sense, Ui};
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

pub fn show(
    ui: &mut Ui,
    state: &ViewState,
    tab: &mut PaneTab,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) {
    let tabs = ["Очередь", "Текст", "Инфо"];
    let idx = match tab {
        PaneTab::Queue => 0,
        PaneTab::Lyrics => 1,
        PaneTab::Info => 2,
    };
    ui.add_space(8.0);
    let w = ((ui.available_width() - 4.0) / 3.0).floor();
    if let Some(i) = ConnectedGroup::new("pane-tabs", &tabs)
        .equal_width(w)
        .show(ui, idx)
    {
        *tab = [PaneTab::Queue, PaneTab::Lyrics, PaneTab::Info][i];
    }
    ui.add_space(12.0);
    match tab {
        PaneTab::Queue => queue(ui, state, chrome, out),
        PaneTab::Lyrics => lyrics(ui, state, out),
        PaneTab::Info => info(ui, state, out),
    }
}

/// A queue row: cover, title and artist.
fn queue_item<'a>(t: &'a TrackRow, salt: impl egui::AsId) -> ListItem<'a> {
    let uri = t.cover.as_ref().map(|c| c.uri());
    ListItem::new(&t.title)
        .supporting(&t.artist)
        .leading(Leading::Image(uri, Icon::MusicNote))
        .id_salt(salt)
}

fn queue(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let q = &state.queue;
    let bottom_h = 56.0;
    let list_h = ui.available_height() - bottom_h;
    egui::ScrollArea::vertical()
        .id_salt("queue-scroll")
        .max_height(list_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(t) = &q.current {
                common::label(ui, "Сейчас играет", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                queue_item(t, "q-current").selected(true).show(ui);
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

fn lyrics(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let l = &state.lyrics;
    if l.loading {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.add(LoadingIndicator::new());
            ui.add_space(8.0);
            common::label(ui, "Ищу текст…", TypeRole::BodyMedium, s.on_surface_variant);
        });
        return;
    }
    if l.lines.is_empty() {
        let text = l.plain.as_deref().unwrap_or("Текста нет");
        egui::ScrollArea::vertical().id_salt("lyrics-plain").show(ui, |ui| {
            ui.label(TypeRole::BodyLarge.style().rich(text, s.on_surface_variant));
        });
        return;
    }
    let pos = state.player.position_now();
    let current = l.lines.iter().rposition(|(t, _)| *t <= pos);
    let area = egui::ScrollArea::vertical()
        .id_salt("lyrics-synced")
        .auto_shrink([false, false]);
    area.show(ui, |ui| {
        let mut target = None;
        for (i, (t, line)) in l.lines.iter().enumerate() {
            let is_cur = Some(i) == current;
            let (style, color) = if is_cur {
                (TypeRole::TitleLarge.emphasized(), s.on_surface)
            } else {
                (TypeRole::BodyLarge.style(), s.on_surface_variant)
            };
            let text = if line.trim().is_empty() { "♪" } else { line.as_str() };
            let r = ui.add(
                egui::Label::new(style.rich(text, color))
                    .wrap()
                    .sense(Sense::click())
                    .selectable(false),
            );
            if r.clicked() {
                out.push(Command::Seek(*t));
            }
            if is_cur {
                target = Some(r.rect);
            }
            ui.add_space(8.0);
        }
        // Follow the current line only when it changes, so manual
        // scrolling is not fought every frame.
        let id = ui.id().with("lyrics-follow");
        let last: Option<usize> = ui.ctx().data(|d| d.get_temp(id));
        if current != last {
            ui.ctx().data_mut(|d| d.insert_temp(id, current));
            if let Some(r) = target {
                ui.scroll_to_rect(r, Some(egui::Align::Center));
            }
        }
    });
    if state.player.status == nikgob_view::Status::Playing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
}

fn info(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
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
