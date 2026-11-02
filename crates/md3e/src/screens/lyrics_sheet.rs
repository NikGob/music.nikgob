//! «Текст песни»: the side sheet behind the lyrics view's ⋮. Every text the
//! track has, each in full under its row; pick one; make line or word
//! timing for it from the recording, watch it being made, check it against
//! the playing song and keep it or drop it (docs/lyrics-align.md).

use egui::{Context, Id, Rect, Sense, Ui, UiBuilder, vec2};
use nikgob_motion::Spring;
use nikgob_view::{AlignStage, Command, LyricLine, LyricsSource, LyricsSync, LyricsText, ViewState};

use crate::components::{
    Button, ListItem, LoadingIndicator, Position, SideSheet, Size, Style, anim,
};
use crate::{Icon, Theme, TypeRole, tokens::shape};

const OPEN: &str = "lyrics-sheet-open";
const EXPANDED: &str = "lyrics-sheet-expanded";

/// Opens the sheet (the ⋮ of the lyrics view).
pub fn open(ctx: &Context) {
    ctx.data_mut(|d| {
        d.insert_temp(Id::new(OPEN), true);
        d.remove::<Option<LyricsSource>>(Id::new(EXPANDED));
    });
}

/// Unfolds `source`'s text.
pub fn expand(ctx: &Context, source: LyricsSource) {
    ctx.data_mut(|d| d.insert_temp(Id::new(EXPANDED), Some(source)));
}

/// Draws the sheet while it is open (or closing).
pub fn show(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    let mut open = ctx.data(|d| d.get_temp::<bool>(Id::new(OPEN))).unwrap_or(false);
    let was = open;
    SideSheet::new("lyrics-sheet").title("Текст песни").width(420.0).show(ctx, &mut open, |ui| {
        egui::ScrollArea::vertical().id_salt("lyrics-sheet-scroll").auto_shrink([false, false]).show(ui, |ui| {
            content(ui, state, out);
        });
    });
    if open != was {
        ctx.data_mut(|d| d.insert_temp(Id::new(OPEN), open));
        // Closing the sheet drops timing made but not kept.
        if !open && state.align.craft.is_some() {
            out.push(Command::KeepCraft(false));
        }
    }
}

/// The row's second line: how the text is timed.
fn timing_label(t: &LyricsText) -> String {
    if t.fits == Some(false) {
        return "Другая запись".to_owned();
    }
    if t.saved {
        return "Время расставлено тобой".to_owned();
    }
    if t.source == Some(LyricsSource::Recognized) {
        return "Распознан по записи".to_owned();
    }
    if t.made.is_some() {
        return "Слова расставлены по записи".to_owned();
    }
    match t.sync.unwrap_or(LyricsSync::Plain) {
        LyricsSync::Syllable => "По слогам",
        LyricsSync::Word => "По словам",
        LyricsSync::Line => "По строкам",
        LyricsSync::Plain => "Без времени",
    }
    .to_owned()
}

fn content(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let l = &state.lyrics;
    let Some(track) = l.track else {
        crate::screens::common::label(ui, "Ничего не играет", TypeRole::BodyLarge, s.on_surface_variant);
        return;
    };
    if l.texts.is_empty() {
        let text = if l.loading || l.checking { "Ищу текст…" } else { "Текста нет нигде" };
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            if l.loading || l.checking {
                ui.add(LoadingIndicator::new().contained(true));
                ui.add_space(12.0);
            }
            crate::screens::common::label(ui, text, TypeRole::BodyLarge, s.on_surface_variant);
        });
        return;
    }
    let expanded_id = Id::new(EXPANDED);
    let expanded: Option<LyricsSource> = ctx.data(|d| d.get_temp::<Option<LyricsSource>>(expanded_id)).unwrap_or(l.source);
    let craft = state.align.craft.as_ref().filter(|c| c.track == track);
    let n = l.texts.len() + 1;
    let old = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;

    // «Автоматически»: the player's own choice.
    let auto = ListItem::new("Автоматически")
        .supporting(l.source.filter(|_| !l.picked).map_or_else(|| "Лучший подходящий текст".to_owned(), |s| s.label().to_owned()))
        .selected(!l.picked)
        .position(Position::of(0, n))
        .fill(s.surface_container_high)
        .id_salt("lyrics-sheet-auto")
        .show(ui);
    if auto.response.clicked() && l.picked {
        out.push(Command::PickLyricsSource(track, None));
    }

    for (i, t) in l.texts.iter().enumerate() {
        let Some(source) = t.source else { continue };
        let open = expanded == Some(source);
        // Only the user's own pick is marked; «Автоматически» says what the
        // player chose.
        let shown = l.picked && l.source == Some(source);
        let chevron = anim::f32(&ctx, Id::new(("lyrics-sheet-chevron", source.key())), if open { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
        let row = ListItem::new(source.label())
            .supporting(timing_label(t))
            .selected(shown)
            .position(Position::of(i + 1, n))
            .fill(s.surface_container_high)
            .id_salt(("lyrics-sheet-row", source.key()))
            .trailing(move |ui: &mut Ui| {
                let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                // The chevron turns over as the text unfolds.
                Icon::ExpandMore
                    .widget(false, Theme::get(ui.ctx()).scheme.on_surface_variant, 24.0)
                    .rotate(std::f32::consts::PI * chevron, vec2(0.5, 0.5))
                    .paint_at(ui, r);
            })
            .show(ui);
        if row.response.clicked() {
            ctx.data_mut(|d| d.insert_temp(expanded_id, if open { None } else { Some(source) }));
        }
        unfold(ui, Id::new(("lyrics-sheet-fold", source.key())), open, |ui| {
            panel(ui, state, track, t, source, craft, out);
        });
    }
    ui.spacing_mut().item_spacing.y = old;
}

/// Content that unfolds on a spatial spring (its height measured the frame
/// before) and fades in on an effects spring.
fn unfold(ui: &mut Ui, id: Id, open: bool, add: impl FnOnce(&mut Ui)) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let k = anim::f32(&ctx, id.with("k"), if open { 1.0 } else { 0.0 }, theme.motion.default_spatial).max(0.0);
    let alpha = anim::f32(&ctx, id.with("a"), if open { 1.0 } else { 0.0 }, theme.motion.default_effects).clamp(0.0, 1.0);
    if k < 0.002 && !open {
        return;
    }
    let full: f32 = ctx.data(|d| d.get_temp(id.with("h"))).unwrap_or(0.0);
    let shown = (full * k).max(0.0);
    let top = ui.cursor().min;
    let width = ui.available_width();
    let clip = Rect::from_min_size(top, vec2(width, shown + 2.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(top, vec2(width, f32::INFINITY))).id_salt(id));
    child.set_clip_rect(clip.intersect(ui.clip_rect()));
    child.multiply_opacity(alpha);
    add(&mut child);
    let h = child.min_rect().height();
    if (h - full).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(id.with("h"), h));
        ctx.request_repaint();
    }
    ui.allocate_rect(Rect::from_min_size(top, vec2(width, shown)), Sense::hover());
}

/// One text unfolded: the text in full, then what can be done with it.
fn panel(ui: &mut Ui, state: &ViewState, track: nikgob_view::TrackId, t: &LyricsText, source: LyricsSource, craft: Option<&nikgob_view::Craft>, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let inner = ui.available_width();
    let frame = egui::Frame::new()
        .fill(s.surface_container_high)
        .corner_radius(shape::EXTRA_SMALL)
        .inner_margin(egui::Margin::same(16));
    frame.show(ui, |ui| {
        ui.set_width(inner - 32.0);
        let mine = craft.filter(|c| c.source == source);
        match mine {
            Some(c) if c.lines.is_none() && c.failed.is_none() => {
                // The text stays in view, dimmed, while its timing is made.
                ui.scope(|ui| {
                    ui.multiply_opacity(0.55);
                    full_text(ui, t, source);
                });
                ui.add_space(16.0);
                making(ui, state);
            }
            Some(c) => craft_view(ui, state, c, out),
            None => {
                full_text(ui, t, source);
                ui.add_space(12.0);
                actions(ui, state, track, t, source, craft.is_some(), out);
            }
        }
    });
    ui.add_space(2.0);
}

fn full_text(ui: &mut Ui, t: &LyricsText, source: LyricsSource) {
    let s = Theme::get(ui.ctx()).scheme;
    let text = if t.lines.is_empty() {
        t.plain.clone().unwrap_or_default()
    } else {
        t.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    };
    egui::ScrollArea::vertical()
        .id_salt(("lyrics-sheet-text", source.key()))
        .max_height(260.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.label(TypeRole::BodyMedium.style().rich_para(text.trim(), s.on_surface));
        });
}

fn actions(ui: &mut Ui, state: &ViewState, track: nikgob_view::TrackId, t: &LyricsText, source: LyricsSource, busy: bool, out: &mut Vec<Command>) {
    let l = &state.lyrics;
    let chosen = l.picked && l.source == Some(source);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let pick = Button::new(if chosen { "Показывается" } else { "Показывать" })
            .icon(Icon::Check)
            .style(if chosen { Style::Tonal } else { Style::Filled })
            .size(Size::S)
            .enabled(!chosen)
            .id_salt(("lyrics-sheet-pick", source.key()));
        if ui.add(pick).clicked() {
            out.push(Command::PickLyricsSource(track, Some(source)));
        }
        let sync = t.sync.unwrap_or(LyricsSync::Plain);
        if source != LyricsSource::Recognized && state.info.path.is_some() {
            let mut targets = Vec::new();
            if sync == LyricsSync::Plain {
                targets.push((LyricsSync::Line, "По строкам"));
            }
            if sync <= LyricsSync::Line || t.made.is_some() {
                targets.push((LyricsSync::Word, if t.made.is_some() { "Заново по словам" } else { "По словам" }));
            }
            for (target, label) in targets {
                let b = Button::new(label)
                    .icon(Icon::GraphicEq)
                    .style(Style::Tonal)
                    .size(Size::S)
                    .enabled(!busy)
                    .id_salt(("lyrics-sheet-craft", source.key(), label));
                if ui.add(b).clicked() {
                    out.push(Command::CraftLyrics { track, source, target });
                }
            }
        }
    });
}

/// The overall share done of a job, from its stage.
fn overall(stage: AlignStage, f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    match stage {
        AlignStage::Download | AlignStage::Recognise => f,
        AlignStage::Decode => 0.03,
        AlignStage::Separate => 0.05 + 0.55 * f,
        AlignStage::Listen => 0.6 + 0.35 * f,
        AlignStage::Align => 0.97,
    }
}

fn stage_label(stage: AlignStage) -> &'static str {
    match stage {
        AlignStage::Download => "Скачиваю модели",
        AlignStage::Decode => "Читаю песню",
        AlignStage::Separate => "Отделяю голос",
        AlignStage::Listen => "Слушаю голос",
        AlignStage::Align => "Расставляю слова",
        AlignStage::Recognise => "Распознаю слова",
    }
}

/// Making, then the result to check.
fn craft_view(ui: &mut Ui, state: &ViewState, c: &nikgob_view::Craft, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    if let Some(e) = &c.failed {
        crate::screens::common::label(ui, e, TypeRole::BodyMedium, s.error);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new("Повторить").style(Style::Tonal).size(Size::S).id_salt("lyrics-sheet-retry")).clicked() {
                out.push(Command::CraftLyrics { track: c.track, source: c.source, target: c.target });
            }
            if ui.add(Button::new("Закрыть").style(Style::Text).size(Size::S).id_salt("lyrics-sheet-fail-close")).clicked() {
                out.push(Command::KeepCraft(false));
            }
        });
        return;
    }
    let Some(lines) = &c.lines else {
        making(ui, state);
        return;
    };
    // The result fades and rises in.
    let k = anim::f32(&ctx, Id::new(("lyrics-sheet-result", c.source.key())), 1.0, theme.motion.default_spatial);
    let lift = (1.0 - k.clamp(0.0, 1.2)) * 16.0;
    ui.add_space(lift.max(0.0));
    // A filled card behind the made timing, which follows the song.
    let card = ui.painter().add(egui::Shape::Noop);
    let preview_w = ui.available_width();
    let r = ui
        .scope(|ui| {
            ui.multiply_opacity(k.clamp(0.0, 1.0));
            crate::screens::lyrics::made_preview(ui, vec2(preview_w, 220.0), state, lines, out);
        })
        .response
        .rect;
    let fill = s.surface_container_highest.gamma_multiply(k.clamp(0.0, 1.0));
    ui.painter().set(card, egui::epaint::RectShape::filled(r, shape::MEDIUM, fill));
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if let Some(first) = lines.iter().find(|l| !l.text.trim().is_empty()).map(|l: &LyricLine| l.start)
            && ui
                .add(
                    crate::components::IconButton::new(Icon::PlayArrow)
                        .style(Style::Tonal)
                        .tooltip("Слушать с начала")
                        .id_salt("lyrics-sheet-listen"),
                )
                .clicked()
        {
            out.push(Command::Seek(first.saturating_sub(std::time::Duration::from_secs(2))));
            out.push(Command::Play);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(Button::new("Сохранить").style(Style::Filled).size(Size::S).id_salt("lyrics-sheet-keep")).clicked() {
                out.push(Command::KeepCraft(true));
            }
            if ui.add(Button::new("Отменить").style(Style::Text).size(Size::S).id_salt("lyrics-sheet-drop")).clicked() {
                out.push(Command::KeepCraft(false));
            }
        });
    });
}

/// The work under way: the Expressive loading shape morphing in its
/// container, the stage, and a wavy bar that moves on a spring.
fn making(ui: &mut Ui, state: &ViewState) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let job = state.align.job.as_ref().filter(|j| j.asked);
    let (stage, f) = job.map_or((AlignStage::Decode, 0.0), |j| (j.stage, j.progress));
    let target = overall(stage, f);
    let shown = anim::f32(&ctx, Id::new("lyrics-sheet-progress"), target, Spring::new(1.0, 120.0)).clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.add(LoadingIndicator::new().contained(true).size(56.0));
        ui.vertical(|ui| {
            ui.add_space(6.0);
            // The stage name cross-fades when it changes.
            let label = stage_label(stage);
            let fade = anim::f32(&ctx, Id::new(("lyrics-sheet-stage", label)), 1.0, theme.motion.default_effects).clamp(0.0, 1.0);
            ui.scope(|ui| {
                ui.multiply_opacity(fade);
                crate::screens::common::label(ui, label, TypeRole::TitleMedium, s.on_surface);
            });
            ui.add_space(8.0);
            let w = ui.available_width();
            crate::components::wavy_progress(ui, "lyrics-sheet-wave", shown, w);
        });
    });
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_only_moves_on_through_the_stages() {
        let order = [
            overall(AlignStage::Decode, 0.0),
            overall(AlignStage::Separate, 0.0),
            overall(AlignStage::Separate, 1.0),
            overall(AlignStage::Listen, 0.5),
            overall(AlignStage::Align, 1.0),
        ];
        assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
        assert!(order.iter().all(|f| (0.0..=1.0).contains(f)));
    }

    #[test]
    fn rows_say_how_a_text_is_timed() {
        let mut t = LyricsText { source: Some(LyricsSource::Lrclib), sync: Some(LyricsSync::Line), ..Default::default() };
        assert_eq!(timing_label(&t), "По строкам");
        t.made = Some(Vec::new());
        assert_eq!(timing_label(&t), "Слова расставлены по записи");
        t.fits = Some(false);
        assert_eq!(timing_label(&t), "Другая запись");
    }
}
