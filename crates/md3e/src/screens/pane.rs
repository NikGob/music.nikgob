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
        Button, ConnectedGroup, IconButton, Leading, ListItem, LoadingIndicator, Menu, MenuItem, Size, Style, Switch,
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

/// The menu of a queue row (its ⋮ and a right click): play it now,
/// «Радио по треку», and take it out of the queue (not the current track,
/// `entry` None).
fn queue_menu(key: impl egui::AsId, entry: Option<u64>) -> Menu {
    let mut m = Menu::new(key).align_right(true);
    if entry.is_some() {
        m = m.item(MenuItem::new("Играть сейчас").icon(Icon::PlayArrow));
    }
    m = m.item(MenuItem::new(super::pages::RADIO_BY_TRACK).icon(Icon::Radio));
    if entry.is_some() {
        m = m.divider().item(MenuItem::new("Убрать из очереди").icon(Icon::Close));
    }
    m
}

fn queue_pick(picked: Option<usize>, t: &TrackRow, entry: Option<u64>, out: &mut Vec<Command>) {
    match (picked, entry) {
        (Some(0), Some(e)) => out.push(Command::PlayQueueEntry(e)),
        (Some(1), Some(_)) | (Some(0), None) => out.push(super::pages::radio_by_track(t.id)),
        (Some(2), Some(e)) => out.push(Command::RemoveFromQueue(e)),
        _ => {}
    }
}

/// A movable queue row (a drag handle; ⋮ and ✕ under the pointer, so the
/// narrow pane keeps the titles whole); played ones are dimmed. Returns its
/// rect and whether its handle is dragged / was released.
fn queue_row(ui: &mut Ui, e: &QueueEntry, played: bool, out: &mut Vec<Command>) -> (Rect, bool, bool) {
    let entry = e.entry;
    // Hovered last frame, or its ⋮ menu is open (the pointer is on the menu).
    let key = ui.id().with(("q-actions", entry));
    let show_actions = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    ui.scope(|ui| {
        if played {
            ui.multiply_opacity(0.6);
        }
        let mut more = None;
        let o = queue_item(&e.track, ("q", entry))
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
        if o.response.double_clicked() {
            out.push(Command::PlayQueueEntry(entry));
        }
        let mut menu_open = false;
        if let Some(m) = &more {
            queue_pick(queue_menu(("q-menu", entry), Some(entry)).attach(m), &e.track, Some(entry), out);
            menu_open = ui.ctx().data(|d| d.get_temp::<bool>(m.id.with("md3e-menu-open"))).unwrap_or(false);
        }
        queue_pick(queue_menu(("q-ctx", entry), Some(entry)).context_menu(&o.response), &e.track, Some(entry), out);
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

pub(super) fn queue(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let q = &state.queue;
    // A radio says so above the list, and where it picks from; autoplay
    // can be switched off right there.
    if let Some(r) = &q.radio {
        if radio_header(ui, r) {
            out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs { autoplay: false, ..state.prefs.clone() })));
        }
        source_row(ui, state, chrome, out);
        ui.add_space(12.0);
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
                // (Sent after the rows: they hold `out` meanwhile.)
                current_pick = queue_menu("q-ctx-current", None).context_menu(&o.response).map(|p| (p, t));
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
                for e in &q.next {
                    row(ui, e, false, &mut rows);
                }
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
            if let Some((p, t)) = current_pick {
                queue_pick(Some(p), t, None, out);
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

/// The end of a queue that is no radio: «Автовоспроизведение» with its
/// switch, and while it is on, where its tracks will come from.
fn autoplay_block(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    let p = &state.prefs;
    ui.add_space(16.0);
    let mut on = p.autoplay;
    let line = if on {
        format!("{} — когда очередь кончится", super::pages::settings_help::radio_source_line(p))
    } else {
        "Очередь закончится на последнем треке".to_owned()
    };
    ListItem::new("Автовоспроизведение")
        .supporting(line)
        .leading_icon(Icon::Radio)
        .trailing(|ui| {
            ui.add(Switch::new(&mut on).id_salt("q-autoplay"));
        })
        .id_salt("q-autoplay-row")
        .show(ui);
    if on != p.autoplay {
        out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs { autoplay: on, ..p.clone() })));
    }
    if p.autoplay {
        source_row(ui, state, chrome, out);
    }
    ui.add_space(8.0);
}

/// Where a radio (or autoplay) takes its tracks: «Скачанное · Библиотека ·
/// С новым» as a connected group bound to the same prefs as Settings, ⚙
/// opening the row there, and a quiet line on what the choice means.
fn source_row(ui: &mut Ui, state: &ViewState, chrome: &mut Chrome, out: &mut Vec<Command>) {
    use super::pages::SettingId;
    let s = Theme::get(ui.ctx()).scheme;
    let p = &state.prefs;
    let labels: Vec<&str> = SettingId::RadioSource.choices().iter().map(|c| c.label).collect();
    let chosen = SettingId::RadioSource.chosen(p);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let settings_w = Size::S.height() + 8.0;
        let w = ((ui.available_width() - 16.0 - settings_w - 4.0) / 3.0).floor().min(140.0);
        let mut gui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_size(ui.cursor().min, egui::vec2(3.0 * w + 4.0, Size::S.height())))
                .id_salt("q-source-row"),
        );
        if let Some(i) = ConnectedGroup::new("q-source", &labels).equal_width(w).show(&mut gui, chosen) {
            let mut prefs = p.clone();
            SettingId::RadioSource.choose(&mut prefs, i);
            out.push(Command::SetPrefs(Box::new(prefs)));
        }
        ui.add_space(3.0 * w + 4.0 + 8.0);
        if ui
            .add(IconButton::new(Icon::Settings).size(Size::S).tooltip("Настроить автовоспроизведение").id_salt("q-autoplay-settings"))
            .clicked()
        {
            chrome.setting = Some(SettingId::Autoplay);
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        let help = SettingId::RadioSource.choices()[chosen].help;
        ui.add(egui::Label::new(TypeRole::BodySmall.style().rich_para(help, s.on_surface_variant)).wrap());
    });
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

/// The radio line above the queue: its title, and that it grows by itself.
/// Autoplay's carries its switch; returns whether it was switched off.
fn radio_header(ui: &mut Ui, r: &nikgob_view::RadioView) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let row = Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), 48.0));
    let icon = Rect::from_center_size(egui::pos2(row.left() + 16.0 + 12.0, row.center().y), egui::vec2(24.0, 24.0));
    Icon::Radio.widget(false, s.on_surface_variant, 24.0).paint_at(ui, icon);
    let text_x = icon.right() + 16.0;
    let mut off = false;
    let mut right = row.right() - 16.0;
    if r.autoplay {
        let slot = Rect::from_min_max(egui::pos2(row.right() - 16.0 - 52.0, row.top()), egui::pos2(row.right() - 16.0, row.bottom()));
        let mut on = true;
        ui.put(slot, Switch::new(&mut on).id_salt("q-radio-autoplay"));
        off = !on;
        right = slot.left() - 16.0;
    }
    let w = (right - text_x).max(1.0);
    let line = |text: &str, role: TypeRole, color| {
        let mut job = egui::text::LayoutJob::single_section(text.to_owned(), role.style().text_format(color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(w);
        ui.painter().layout_job(job)
    };
    // Autoplay names what it continues.
    let (title, sub) = if r.autoplay {
        (r.title.clone(), "Автовоспроизведение: дописывается, пока играет")
    } else {
        (radio_title(&r.title), "Дописывается сама, пока играет")
    };
    let title = line(&title, TypeRole::TitleSmall, s.on_surface);
    let sub = line(sub, TypeRole::BodySmall, s.on_surface_variant);
    let top = row.center().y - (title.size().y + sub.size().y) * 0.5;
    let sub_y = top + title.size().y;
    ui.painter().galley(egui::pos2(text_x, top), title, s.on_surface);
    ui.painter().galley(egui::pos2(text_x, sub_y), sub, s.on_surface_variant);
    ui.advance_cursor_after_rect(row);
    off
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
        egui::pos2(row.left() + 16.0 + 56.0 + 16.0, row.center().y),
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
                // A clip: its song by hand; a stream from YouTube: another version.
                if let Some(find) = super::find_song::label(t) {
                    ui.add_space(8.0);
                    if ui
                        .add(Button::new(find).icon(Icon::MusicNote).style(Style::Outlined).size(Size::S).id_salt("info-find-song"))
                        .clicked()
                    {
                        out.push(Command::FindSong { track: t.id, query: None });
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_queue_is_named_and_its_menu_starts_a_radio() {
        assert_eq!(radio_title("Радио · Кино"), "Радио · Кино");
        assert_eq!(radio_title("Спокойное"), "Микс · Спокойное");
        let t = TrackRow {
            id: nikgob_view::TrackId::new(),
            title: "Группа крови".into(),
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
        };
        let radio = Command::StartRadio { seed: nikgob_view::RadioSeed::Track(t.id) };
        // A row of the queue: play now, the radio, take it out.
        let mut out = Vec::new();
        for i in 0..3 {
            queue_pick(Some(i), &t, Some(9), &mut out);
        }
        assert_eq!(out, vec![Command::PlayQueueEntry(9), radio.clone(), Command::RemoveFromQueue(9)]);
        // The current track: only the radio.
        let mut out = Vec::new();
        queue_pick(Some(0), &t, None, &mut out);
        assert_eq!(out, vec![radio]);
    }
}
