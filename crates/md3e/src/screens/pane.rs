//! The supporting pane: Очередь | Текст | Инфо (md3e-player.md §2.4).

use egui::{Rect, Ui};
use nikgob_view::{Command, QueueEntry, TrackRow, ViewState};

use super::{
    PaneTab,
    chrome::Chrome,
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

/// A movable queue row (✕ and a drag handle); played ones are dimmed.
/// Returns its rect and whether its handle is dragged / was released.
fn queue_row(ui: &mut Ui, e: &QueueEntry, played: bool, out: &mut Vec<Command>) -> (Rect, bool, bool) {
    let entry = e.entry;
    ui.scope(|ui| {
        if played {
            ui.multiply_opacity(0.6);
        }
        let o = queue_item(&e.track, ("q", entry))
            .muted(played)
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
        if o.response.double_clicked() {
            out.push(Command::PlayQueueEntry(entry));
        }
        let (drag, drop) = o.handle.as_ref().map_or((false, false), |h| (h.dragged(), h.drag_stopped()));
        (o.rect, drag, drop)
    })
    .inner
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
            // The queue is a temporary playlist: every row but the current
            // one can be removed or dragged anywhere, across the current
            // track too (a played one back up next, an upcoming one into
            // the history). Rows in queue order: (entry, rect).
            let mut rows: Vec<(u64, Rect)> = Vec::new();
            // Index into `rows` of the row whose handle is dragged / was released.
            let mut dragging = None;
            let mut dropped = None;
            let mut row = |ui: &mut Ui, e: &QueueEntry, played: bool, rows: &mut Vec<(u64, Rect)>| {
                let (rect, drag, drop) = queue_row(ui, e, played, out);
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
                for e in &q.prev {
                    row(ui, e, true, &mut rows);
                }
                ui.add_space(12.0);
            }
            if let Some(t) = &q.current {
                let top = ui.cursor().min.y;
                common::label(ui, "Сейчас играет", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                let o = queue_item(t, "q-current").selected(true).show(ui);
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
            if q.next.is_empty() {
                common::label(ui, "Дальше ничего нет", TypeRole::BodyMedium, s.on_surface_variant);
            } else {
                common::label(ui, "Далее", TypeRole::TitleSmall, s.on_surface_variant);
                ui.add_space(4.0);
                for e in &q.next {
                    row(ui, e, false, &mut rows);
                }
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
        match &state.player.current {
            None => {
                common::label(ui, "Ничего не играет", TypeRole::BodyMedium, s.on_surface_variant);
            }
            Some(t) => {
                ui.add_space(12.0);
                // A file has tags to edit; a stream can be downloaded.
                let (label, icon) = if t.has_local { ("Редактировать теги…", Icon::Edit) } else { ("Скачать", Icon::Download) };
                if ui
                    .add(Button::new(label).icon(icon).style(Style::Outlined).size(Size::S).id_salt("info-tags"))
                    .clicked()
                {
                    out.push(if t.has_local { Command::OpenTagEditor(t.id) } else { Command::DownloadTracks(vec![t.id]) });
                }
                ui.add_space(8.0);
            }
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
    #[cfg(not(windows))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
