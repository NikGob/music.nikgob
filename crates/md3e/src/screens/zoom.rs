//! «Масштаб интерфейса»: the prefs' scale as egui's zoom factor, on top of
//! the system's own scale, and Ctrl+= / Ctrl+- / Ctrl+0 to change it a
//! step at a time (saved in the prefs like the row in Настройки → Вид).
//!
//! egui's own zoom keys are turned off: they go past the range and are
//! never saved.

use egui::{Context, Id, Key, Modifiers};
use nikgob_view::{Command, PrefPatch, Prefs};

/// A scale set from the keys is used until the prefs have it, at most this
/// long (a change the runtime did not take goes back).
const PENDING_FOR: f64 = 2.0;

/// The scale the keys set and when, while the prefs do not have it yet
/// (a second key press before the runtime answered steps from it).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pending {
    scale: f32,
    at: f64,
}

fn id() -> Id {
    Id::new("md3e-ui-scale")
}

/// Takes the zoom keys of this frame and sets the window's zoom to the
/// scale: the one just set by a key, else the prefs'.
pub fn apply(ctx: &Context, prefs: &Prefs, out: &mut Vec<Command>) {
    if ctx.options(|o| o.zoom_with_keyboard) {
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
    }
    let now = ctx.input(|i| i.time);
    let saved = nikgob_view::ui_scale(prefs.ui_scale);
    let mut pending = ctx.data(|d| d.get_temp::<Pending>(id())).filter(|p| p.scale != saved && now - p.at < PENDING_FOR);
    let current = pending.map_or(saved, |p| p.scale);
    // A text field being typed in keeps its keys (the scale stays as set).
    let typing = ctx.egui_wants_keyboard_input();
    let (up, down, reset) = if typing {
        (0, 0, 0)
    } else {
        ctx.input_mut(|i| {
            (
                i.count_and_consume_key(Modifiers::COMMAND, Key::Equals) + i.count_and_consume_key(Modifiers::COMMAND, Key::Plus),
                i.count_and_consume_key(Modifiers::COMMAND, Key::Minus),
                i.count_and_consume_key(Modifiers::COMMAND, Key::Num0),
            )
        })
    };
    if up + down + reset > 0 {
        let next = if reset > 0 { 1.0 } else { nikgob_view::ui_scale_step(current, up as i32 - down as i32) };
        if next != current {
            pending = Some(Pending { scale: next, at: now });
            out.push(Command::SetPref(PrefPatch::UiScale(next)));
        }
    }
    ctx.data_mut(|d| match pending {
        Some(p) => {
            d.insert_temp(id(), p);
        }
        None => d.remove::<Pending>(id()),
    });
    let target = pending.map_or(saved, |p| p.scale);
    if ctx.zoom_factor() != target {
        ctx.set_zoom_factor(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, RawInput};

    fn press(key: Key) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }
    }

    /// One frame at `t` with `events`: what it sent and the zoom after it.
    fn frame(ctx: &Context, prefs: &Prefs, events: Vec<Event>, t: f64) -> (Vec<Command>, f32) {
        let mut out = Vec::new();
        let _ = ctx.run_ui(RawInput { time: Some(t), events, ..Default::default() }, |ui| apply(ui.ctx(), prefs, &mut out));
        (out, ctx.zoom_factor())
    }

    fn scales(out: &[Command]) -> Vec<f32> {
        out.iter()
            .filter_map(|c| match c {
                Command::SetPref(PrefPatch::UiScale(s)) => Some(*s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_prefs_scale_is_the_zoom() {
        let ctx = Context::default();
        let mut prefs = Prefs::default();
        let (out, _) = frame(&ctx, &prefs, Vec::new(), 0.0);
        assert!(out.is_empty());
        assert!(!ctx.options(|o| o.zoom_with_keyboard), "egui's own keys are off");
        prefs.ui_scale = 1.3;
        frame(&ctx, &prefs, Vec::new(), 0.1);
        let (_, zoom) = frame(&ctx, &prefs, Vec::new(), 0.2);
        assert_eq!(zoom, 1.3);
        // A value off its steps (a hand-edited file): the nearest step.
        prefs.ui_scale = 7.0;
        frame(&ctx, &prefs, Vec::new(), 0.3);
        let (_, zoom) = frame(&ctx, &prefs, Vec::new(), 0.4);
        assert_eq!(zoom, 1.5);
    }

    #[test]
    fn the_keys_step_the_scale_and_save_it() {
        let ctx = Context::default();
        let mut prefs = Prefs::default();
        frame(&ctx, &prefs, Vec::new(), 0.0);
        let (out, _) = frame(&ctx, &prefs, vec![press(Key::Equals)], 0.1);
        assert_eq!(scales(&out), [1.1]);
        // The runtime has not answered yet: the next press steps on from 1.1.
        let (out, _) = frame(&ctx, &prefs, vec![press(Key::Plus)], 0.2);
        assert_eq!(scales(&out), [1.2]);
        let (_, zoom) = frame(&ctx, &prefs, Vec::new(), 0.3);
        assert_eq!(zoom, 1.2, "the window follows at once");
        prefs.ui_scale = 1.2;
        let (out, _) = frame(&ctx, &prefs, vec![press(Key::Minus), press(Key::Minus)], 0.4);
        assert_eq!(scales(&out), [1.0]);
        prefs.ui_scale = 1.0;
        frame(&ctx, &prefs, Vec::new(), 0.45);
        // At the ends nothing is sent.
        prefs.ui_scale = 0.8;
        let (out, _) = frame(&ctx, &prefs, vec![press(Key::Minus)], 0.5);
        assert!(scales(&out).is_empty());
        let (out, _) = frame(&ctx, &prefs, vec![press(Key::Num0)], 0.6);
        assert_eq!(scales(&out), [1.0]);
        // A change the prefs never took goes back after a while.
        let (_, zoom) = frame(&ctx, &prefs, Vec::new(), 0.7);
        assert_eq!(zoom, 1.0);
        frame(&ctx, &prefs, Vec::new(), 0.6 + PENDING_FOR + 0.1);
        let (_, zoom) = frame(&ctx, &prefs, Vec::new(), 0.6 + PENDING_FOR + 0.2);
        assert_eq!(zoom, 0.8);
    }

    /// While a text field is typed in, the zoom keys are its own: the
    /// scale stays and the key is left for the field.
    #[test]
    fn a_focused_text_field_keeps_the_keys() {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let prefs = Prefs::default();
        let mut text = String::new();
        let mut run = |events: Vec<Event>, t: f64| {
            let mut out = Vec::new();
            let mut left = 0;
            let _ = ctx.run_ui(RawInput { time: Some(t), events, ..Default::default() }, |ui| {
                apply(ui.ctx(), &prefs, &mut out);
                left = ui.ctx().input(|i| i.events.iter().filter(|e| matches!(e, Event::Key { key: Key::Equals, .. })).count());
                ui.add(egui::TextEdit::singleline(&mut text).id(Id::new("zoom-test-field"))).request_focus();
            });
            (out, left)
        };
        run(Vec::new(), 0.0);
        let (out, left) = run(vec![press(Key::Equals)], 0.1);
        assert!(scales(&out).is_empty(), "no scale change while typing");
        assert_eq!(left, 1, "the key is left for the field");
    }
}
