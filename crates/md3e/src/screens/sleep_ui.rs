//! «Таймер сна» in the player: the choices of the ⋮ submenu and, while a
//! timer runs, its chip (the moon and the time left), which opens the same
//! choices. T steps through the timers of minutes ([`nikgob_view::sleep_key_step`]).

use std::time::{Duration, Instant};

use egui::{Color32, Id, Rect, Response, Sense, Ui, text::LayoutJob, vec2};
use nikgob_view::{Command, SLEEP_MINUTES, SleepChoice, SleepContext, ViewState, sleep_left_label};

use super::{common::plural, pages::widgets::Row};
use crate::{
    Icon, Theme, TypeRole,
    components::{MenuItem, Radii, anim, paint, tooltip::tooltip},
};

/// The row of the player's ⋮ that opens the choices.
pub const MENU_LABEL: &str = "Таймер сна";
/// The timer's key.
pub const KEY: &str = "T";

const CHIP_H: f32 = 32.0;
const ICON: f32 = 18.0;
const PAD: f32 = 12.0;
const GAP: f32 = 6.0;

/// What a choice is called in the menu.
pub fn choice_label(choice: SleepChoice, context: SleepContext) -> String {
    match choice {
        SleepChoice::Minutes(m) if m % 60 == 0 => {
            let h = u64::from(m / 60);
            format!("{h} {}", plural(h, "час", "часа", "часов"))
        }
        SleepChoice::Minutes(m) if m > 60 && m % 30 == 0 => format!("{},5 часа", m / 60),
        SleepChoice::Minutes(m) => format!("{m} {}", plural(u64::from(m), "минута", "минуты", "минут")),
        SleepChoice::TrackEnd => "До конца трека".to_owned(),
        SleepChoice::ContextEnd => match context {
            SleepContext::Album => "До конца альбома".to_owned(),
            SleepContext::Playlist => "До конца плейлиста".to_owned(),
            SleepContext::Queue => "До конца очереди".to_owned(),
        },
        SleepChoice::Off => "Выключить таймер".to_owned(),
    }
}

/// The choices as menu rows (`wrap` makes each the menu's action): the
/// running one selected; the ends of a track and of the album while
/// something plays; «Выключить таймер» while a timer runs.
pub fn rows<A>(state: &ViewState, wrap: impl Fn(SleepChoice) -> A) -> Vec<Row<A>> {
    let p = &state.player;
    let running = p.sleep.as_ref().map(|s| s.choice);
    let context = SleepContext::of(p);
    let item = |c: SleepChoice| Row::Item(wrap(c), MenuItem::new(choice_label(c, context)).selected(running == Some(c)));
    let mut rows: Vec<Row<A>> = SLEEP_MINUTES.iter().map(|m| item(SleepChoice::Minutes(*m))).collect();
    if p.current.is_some() {
        rows.push(Row::Divider);
        rows.push(item(SleepChoice::TrackEnd));
        rows.push(item(SleepChoice::ContextEnd));
    }
    if running.is_some() {
        rows.push(Row::Divider);
        rows.push(Row::Item(wrap(SleepChoice::Off), MenuItem::new(choice_label(SleepChoice::Off, context)).icon(Icon::Close)));
    }
    rows
}

/// The ⋮ row that opens the choices.
pub fn submenu<A>(state: &ViewState, wrap: impl Fn(SleepChoice) -> A) -> Row<A> {
    let item = MenuItem::new(MENU_LABEL).icon(Icon::Bedtime).selected(state.player.sleep.is_some());
    Row::Sub(item, rows(state, wrap))
}

/// What the chip says now, while a timer runs.
fn left_label(state: &ViewState) -> Option<String> {
    let s = state.player.sleep.as_ref()?;
    Some(sleep_left_label(s.left(&state.player, Instant::now())))
}

/// The chip's room in the row now (it grows in on a spring when a timer
/// starts and shrinks away when it ends): what a row lays out around it.
pub fn chip_width(ui: &Ui, state: &ViewState, salt: &str) -> f32 {
    let (k, label) = presence(ui, state, salt);
    if k <= 0.0 { 0.0 } else { full_width(ui, &label) * k + GAP * k.min(1.0) }
}

/// How far in the chip is (0 gone, 1 in, a little over on the spring) and
/// what it says (the last words while it leaves).
fn presence(ui: &Ui, state: &ViewState, salt: &str) -> (f32, String) {
    let id = Id::new(("md3e-sleep-chip", salt));
    let ctx = ui.ctx();
    let label = left_label(state);
    let last = ctx.data_mut(|d| {
        let last = d.get_temp_mut_or_insert_with(id.with("label"), String::new);
        if let Some(l) = &label {
            l.clone_into(last);
        }
        last.clone()
    });
    let motion = Theme::get(ctx).motion;
    let k = anim::f32(ctx, id.with("k"), if label.is_some() { 1.0 } else { 0.0 }, motion.fast_spatial);
    (k.max(0.0), last)
}

fn galley(ui: &Ui, text: &str, color: Color32) -> std::sync::Arc<egui::Galley> {
    let job = LayoutJob::single_section(text.to_owned(), TypeRole::LabelLarge.style().text_format(color));
    ui.painter().layout_job(job)
}

fn full_width(ui: &Ui, label: &str) -> f32 {
    PAD + ICON + GAP + galley(ui, label, Color32::WHITE).size().x + PAD
}

/// The chip of a running timer in `container` / `content` colours (the
/// player's own); a click opens the choices. Nothing while no timer runs
/// (once it has shrunk away).
pub fn chip(ui: &mut Ui, state: &ViewState, salt: &str, container: Color32, content: Color32, out: &mut Vec<Command>) {
    let (k, label) = presence(ui, state, salt);
    if k <= 0.001 && state.player.sleep.is_none() {
        return;
    }
    let ctx = ui.ctx().clone();
    // The time left changes by itself, paused or not.
    ctx.request_repaint_after(Duration::from_secs(1));
    let full = full_width(ui, &label);
    let (slot, resp) = ui.allocate_exact_size(vec2(full * k + GAP * k.min(1.0), CHIP_H), Sense::click());
    let alpha = anim::fraction(&ctx, Id::new(("md3e-sleep-chip", salt, "a")), if state.player.sleep.is_some() { 1.0 } else { 0.0 }, Theme::get(&ctx).motion.fast_effects);
    // Grows from its centre; the room it takes follows the same spring.
    let scale = k.clamp(0.0, 1.2);
    let rect = Rect::from_center_size(slot.center(), vec2(full * scale, CHIP_H * scale.min(1.0)));
    let painter = ui.painter_at(slot.expand(4.0));
    let radii = Radii::all(CHIP_H / 2.0);
    paint::container(&painter, rect, radii, container.gamma_multiply(alpha), egui::Stroke::NONE);
    if resp.hovered() {
        paint::state_layer(&painter, rect, radii, content, 0.08 * alpha);
    }
    let fg = content.gamma_multiply(alpha);
    if scale > 0.5 {
        let text = galley(ui, &label, fg);
        let left = rect.center().x - (ICON + GAP + text.size().x) / 2.0;
        let icon = Rect::from_min_size(egui::pos2(left, rect.center().y - ICON / 2.0), vec2(ICON, ICON));
        Icon::Bedtime.widget(true, fg, ICON).paint_at(ui, icon);
        painter.galley(egui::pos2(icon.right() + GAP, rect.center().y - text.size().y / 2.0), text, fg);
    }
    if state.player.sleep.is_none() {
        return;
    }
    let resp = tooltip(resp, MENU_LABEL, Some(KEY));
    menu(&resp, state, salt, out);
}

/// The choices on `anchor`, right-aligned.
fn menu(anchor: &Response, state: &ViewState, salt: &str, out: &mut Vec<Command>) {
    let (menu, picks) = super::pages::widgets::menu_of(("md3e-sleep-menu", salt), rows(state, |c| c));
    if let Some(choice) = menu.align_right(true).attach(anchor).and_then(|i| picks.get(i).copied()) {
        out.push(Command::SleepTimer(choice));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::SleepView;

    fn labels(rows: &[Row<SleepChoice>]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Item(_, item) => Some(item.label.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_choices_read_in_russian() {
        let album = SleepContext::Album;
        assert_eq!(choice_label(SleepChoice::Minutes(15), album), "15 минут");
        assert_eq!(choice_label(SleepChoice::Minutes(30), album), "30 минут");
        assert_eq!(choice_label(SleepChoice::Minutes(60), album), "1 час");
        assert_eq!(choice_label(SleepChoice::Minutes(90), album), "1,5 часа");
        assert_eq!(choice_label(SleepChoice::ContextEnd, album), "До конца альбома");
        assert_eq!(choice_label(SleepChoice::ContextEnd, SleepContext::Playlist), "До конца плейлиста");
    }

    #[test]
    fn off_only_while_a_timer_runs_and_the_ends_only_while_playing() {
        let mut state = ViewState::default();
        assert_eq!(labels(&rows(&state, |c| c)), ["15 минут", "30 минут", "45 минут", "1 час", "1,5 часа"]);
        state.player.current = Some(nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(),
            title: "Песня".into(),
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
        });
        state.player.sleep = Some(SleepView { choice: SleepChoice::TrackEnd, deadline: None, after_current: Duration::ZERO });
        let rows = rows(&state, |c| c);
        let names = labels(&rows);
        assert_eq!(names[5..], ["До конца трека", "До конца очереди", "Выключить таймер"]);
        let picked = rows.iter().find_map(|r| match r {
            Row::Item(c, item) if item.selected => Some(*c),
            _ => None,
        });
        assert_eq!(picked, Some(SleepChoice::TrackEnd));
    }
}
