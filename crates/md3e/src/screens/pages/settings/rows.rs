//! What every settings tab is built from: groups that show only with a
//! row in them, the row's grid (name · control · value), the levels a row
//! shows at, and the small controls the tabs share.
//!
//! A row draws only at its [`Level`]: «Просто» draws `Simple`, «Подробно»
//! adds `Deep`, and the owner's own builds `Dev` in «Подробно». A group's
//! title (and a subtitle in «Подробно») waits until its first row that
//! shows, so a group with nothing to show has no title either.
//!
//! The grid: the name (40 % of the row, 180–320, with its ⓘ) · the control,
//! its left edge on one line in every row · for a slider the exact value in
//! a small field at the row's right (a control: type a number and press
//! Enter). Dependent rows stand 16 further in, faded while what they tune
//! is off (they still work). Text beside a control is only an error, a
//! blocker or a value (a path, an account): no explanations, which live in
//! the ⓘ cards.

use egui::{Sense, Stroke, Ui, vec2};
use nikgob_view::Prefs;

use super::super::settings_help::{self, Level, SettingId as Id};
use super::super::{brand, connected};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, FilterChip, Menu, MenuItem, Slider, Style, Switch},
};

/// What the page shows this pass: «Подробно», the developer rows, and the
/// page's own opacity (a header drawn from inside a faded row keeps it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mode {
    pub deep: bool,
    pub dev: bool,
    pub opacity: f32,
}

impl Default for Mode {
    /// Outside the page (a test drawing one block): everything there is.
    fn default() -> Self {
        Self { deep: true, dev: brand::own_build(), opacity: 1.0 }
    }
}

fn mode_key() -> egui::Id {
    egui::Id::new("settings-mode")
}

pub(super) fn set_mode(ctx: &egui::Context, mode: Mode) {
    ctx.data_mut(|d| d.insert_temp(mode_key(), mode));
}

pub(crate) fn mode(ctx: &egui::Context) -> Mode {
    ctx.data(|d| d.get_temp::<Mode>(mode_key())).unwrap_or_default()
}

/// Whether `level` shows under `mode`.
pub(crate) fn level_shown(mode: Mode, level: Level) -> bool {
    match level {
        Level::Simple => true,
        Level::Deep => mode.deep,
        Level::Dev => mode.deep && mode.dev,
    }
}

/// Whether the row of `id` shows this pass.
pub(crate) fn shown(ctx: &egui::Context, id: Id) -> bool {
    level_shown(mode(ctx), id.level())
}

/// A group's title and a subtitle, waiting for the first row that shows.
#[derive(Clone, Debug, Default)]
struct Pending {
    group: Option<String>,
    sub: Option<String>,
}

fn pending_key() -> egui::Id {
    egui::Id::new("settings-pending-title")
}

/// Starts a group titled `title`.
pub(crate) fn group(ui: &mut Ui, title: &str) {
    ui.ctx().data_mut(|d| d.insert_temp(pending_key(), Pending { group: Some(title.to_owned()), sub: None }));
}

/// A subtitle inside a big group: «Подробно» only (in «Просто» the few
/// rows of the group need none).
pub(crate) fn sub(ui: &mut Ui, title: &str) {
    if !mode(ui.ctx()).deep {
        return;
    }
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Pending>(pending_key()).sub = Some(title.to_owned()));
}

/// Draws the titles waiting for this row; a group titled `same` (a list
/// that draws its own title) is only started.
fn flush(ui: &mut Ui, same: Option<&str>) {
    let ctx = ui.ctx().clone();
    let p = ctx.data_mut(|d| d.remove_temp::<Pending>(pending_key())).unwrap_or_default();
    if p.group.is_none() && p.sub.is_none() {
        return;
    }
    let m = mode(&ctx);
    let s = Theme::get(&ctx).scheme;
    ui.scope(|ui| {
        // Not faded with a dependent row, not moved in with it.
        ui.set_opacity(m.opacity);
        if let Some(title) = &p.group {
            settings_help::group_start(ui);
            settings_help::note_group(&ctx, title);
            if same != Some(title.as_str()) {
                ui.add_space(24.0);
                ui.label(TypeRole::TitleSmall.style().rich(title, s.primary));
                ui.add_space(4.0);
            }
        }
        if let Some(title) = &p.sub {
            ui.add_space(12.0);
            ui.label(TypeRole::LabelLarge.style().rich(title, s.on_surface_variant));
            ui.add_space(2.0);
        }
    });
}

fn indent_key() -> egui::Id {
    egui::Id::new("settings-indent")
}

fn indent(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(indent_key())).unwrap_or(0.0)
}

/// Rows that depend on the row above: 16 further in, faded while `dim`.
pub(crate) fn child(ui: &mut Ui, dim: bool, add: impl FnOnce(&mut Ui)) {
    let ctx = ui.ctx().clone();
    let was = indent(&ctx);
    ctx.data_mut(|d| d.insert_temp(indent_key(), was + 16.0));
    dimmed(ui, dim, add);
    ctx.data_mut(|d| d.insert_temp(indent_key(), was));
}

/// Rows drawn faded while what they tune is off (they still work).
pub(crate) fn dimmed(ui: &mut Ui, dim: bool, add: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        if dim {
            ui.multiply_opacity(0.55);
        }
        add(ui);
    });
}

/// One settings item (a row, or a block of fields): drawn only at its
/// level, after the titles waiting for it, with its glow and ⓘ.
pub(crate) fn item<R>(ui: &mut Ui, id: Id, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    if !shown(ui.ctx(), id) {
        return None;
    }
    flush(ui, None);
    Some(settings_help::item(ui, id, id.label(), add))
}

/// A list (or a block) under its own title, which carries its ⓘ; a group
/// of the same name is not titled twice.
pub(crate) fn titled<R>(ui: &mut Ui, id: Id, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    if !shown(ui.ctx(), id) {
        return None;
    }
    flush(ui, Some(id.label()));
    Some(settings_help::item(ui, id, id.label(), |ui| {
        list_title(ui, id);
        add(ui)
    }))
}

/// Text beside a row's name: only what is wrong, what blocks it, or a
/// value that is not a number (a path, an account).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Note<'a> {
    Error(&'a str),
    Blocker(&'a str),
    Value(&'a str),
}

/// A 56 row: its name with its ⓘ, the control.
pub(crate) fn row(ui: &mut Ui, id: Id, control: impl FnOnce(&mut Ui)) {
    row_full(ui, id, None, control, None);
}

/// A row with a [`Note`] under its name (72 high).
pub(crate) fn row_note(ui: &mut Ui, id: Id, note: Option<Note<'_>>, control: impl FnOnce(&mut Ui)) {
    row_full(ui, id, note, control, None);
}

fn row_full(
    ui: &mut Ui,
    id: Id,
    note: Option<Note<'_>>,
    control: impl FnOnce(&mut Ui),
    value: Option<&mut dyn FnMut(&mut Ui)>,
) {
    let _ = item(ui, id, |ui| row_body(ui, id, note, control, value));
}

/// The row's name with its ⓘ right after the last word.
fn row_label(ui: &mut Ui, id: Id) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(TypeRole::BodyLarge.style().rich(id.label(), s.on_surface));
        if id.beta() {
            beta_mark(ui);
        }
        settings_help::help_button(ui, id);
    });
}

fn note_label(ui: &mut Ui, note: Note<'_>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (text, color) = match note {
        Note::Error(t) => (t, s.error),
        Note::Blocker(t) | Note::Value(t) => (t, s.on_surface_variant),
    };
    ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(text, color)).truncate());
}

/// «Бета» after a row's label: a 16 dp outlined label-small mark.
fn beta_mark(ui: &mut Ui) {
    let s = Theme::get(ui.ctx()).scheme;
    let galley = ui.painter().layout_no_wrap("Бета".to_owned(), TypeRole::LabelSmall.font_id(), s.tertiary);
    let size = vec2(galley.size().x + 12.0, 16.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_stroke(rect, rect.height() * 0.5, Stroke::new(1.0, s.tertiary), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() * 0.5, galley, s.tertiary);
}

/// The control, reported so the row's cards line up with it. Beside the
/// text its width is kept for the next frame's layout (under the text a
/// control may stretch, which says nothing about the room it needs).
fn row_control(ui: &mut Ui, id: Id, beside: bool, control: impl FnOnce(&mut Ui)) {
    let r = ui.scope(control).response.rect;
    settings_help::report_control(ui.ctx(), r);
    if beside {
        ui.ctx().data_mut(|d| d.insert_temp(control_width_key(id), r.width()));
    }
}

fn control_width_key(id: Id) -> egui::Id {
    egui::Id::new(("settings-control-w", id))
}

/// The width of the name's column of a row `width` wide (its ⓘ included).
pub(crate) fn label_column(width: f32) -> f32 {
    (width * 0.4).clamp(180.0, 320.0) + settings_help::BTN + 4.0
}

/// The exact value's field at a slider row's right.
pub(crate) const VALUE_W: f32 = 60.0;
/// Between a control and the value's field.
const VALUE_GAP: f32 = 12.0;

fn row_body(ui: &mut Ui, id: Id, note: Option<Note<'_>>, control: impl FnOnce(&mut Ui), value: Option<&mut dyn FnMut(&mut Ui)>) {
    let indent = indent(ui.ctx());
    let full = ui.available_width();
    let label_w = label_column(full);
    // A control wider than the room beside the name goes under it, as in a
    // narrow page (three segments do not fit beside the name at 480 px).
    let control_w: f32 = ui.ctx().data(|d| d.get_temp(control_width_key(id))).unwrap_or(0.0);
    let value_w = if value.is_some() { VALUE_W + VALUE_GAP } else { 0.0 };
    let needed = label_w + 16.0 + control_w + value_w;
    if full < 360.0 || needed > full + 1.0 {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.vertical(|ui| {
                row_label(ui, id);
                if let Some(n) = note {
                    note_label(ui, n);
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(indent);
            row_control(ui, id, false, control);
            if let Some(v) = value {
                ui.add_space(VALUE_GAP);
                v(ui);
            }
        });
        ui.add_space(8.0);
        return;
    }
    let h = if note.is_some() { 72.0 } else { 56.0 };
    ui.allocate_ui_with_layout(vec2(full, h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(indent);
        ui.vertical(|ui| {
            // The control's left edge stays on the line of the rows above.
            ui.set_width((label_w - indent).max(80.0));
            // Centre the text block against the control.
            let block = if note.is_some() { 44.0 } else { 24.0 };
            ui.add_space(((h - block) * 0.5).max(0.0));
            row_label(ui, id);
            if let Some(n) = note {
                note_label(ui, n);
            }
        });
        ui.add_space(16.0);
        row_control(ui, id, true, control);
        if let Some(v) = value {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| v(ui));
        }
    });
}

/// A switch row.
pub(crate) fn switch(ui: &mut Ui, id: Id, on: &mut bool) {
    row(ui, id, |ui| {
        ui.add(Switch::new(on).id_salt(("set-switch", id)));
    });
}

/// A switch row that may be off for now (it keeps its value).
pub(crate) fn switch_enabled(ui: &mut Ui, id: Id, on: &mut bool, enabled: bool) {
    row(ui, id, |ui| {
        ui.add(Switch::new(on).enabled(enabled).id_salt(("set-switch", id)));
    });
}

/// A row of segmented buttons ([`Id::choices`]).
pub(crate) fn segments(ui: &mut Ui, id: Id, p: &mut Prefs) {
    row(ui, id, |ui| choice(ui, id, p));
}

/// A row with a menu chip ([`Id::choices`], long labels).
pub(crate) fn menu(ui: &mut Ui, id: Id, p: &mut Prefs) {
    row(ui, id, |ui| choice_menu(ui, id, p));
}

/// Segmented buttons for a choice row (labels and values from
/// [`Id::choices`]); their rects go to the ⓘ card (and the stage).
pub(crate) fn choice(ui: &mut Ui, id: Id, p: &mut Prefs) {
    let labels: Vec<&str> = id.choices().iter().map(|c| c.label).collect();
    let (pick, rects) = connected(ui, egui::Id::new(("set-choice", id)), &labels, id.chosen(p), &[]);
    settings_help::report_choices(ui.ctx(), rects);
    if let Some(n) = pick {
        id.choose(p, n);
    }
}

/// A menu chip for a choice row with long labels.
pub(crate) fn choice_menu(ui: &mut Ui, id: Id, p: &mut Prefs) {
    let cur = id.chosen(p);
    let options: Vec<(String, bool)> = id.choices().iter().enumerate().map(|(i, c)| (c.label.to_owned(), i == cur)).collect();
    let salt = format!("set-menu-{id:?}");
    let pick = dropdown(ui, &salt, id.choices()[cur].label, &options);
    // The open menu's items, for the ⓘ card (and the stage).
    if let Some(rects) = Menu::item_rects(ui.ctx(), (salt.as_str(), "menu")) {
        settings_help::report_menu(ui.ctx(), rects);
    }
    if let Some(n) = pick {
        id.choose(p, n);
    }
}

/// A chip that opens a menu of `options` (label, selected). Returns the pick.
pub(crate) fn dropdown(ui: &mut Ui, id: &str, current: &str, options: &[(String, bool)]) -> Option<usize> {
    let resp = ui.add(FilterChip::new(ellipsize(current, 30), false).trailing_icon(Icon::ExpandMore).id_salt(id));
    let mut menu = Menu::new((id, "menu"));
    for (label, selected) in options {
        menu = menu.item(MenuItem::new(label.as_str()).selected(*selected));
    }
    menu.attach(&resp)
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

/// A number a slider sets: its range and step, and how its value reads in
/// the field («48 с», «65 %», «+0,20 с», «∞»).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Num {
    pub lo: f32,
    pub hi: f32,
    pub step: f32,
    /// The shown number is the value times this (a share as percent).
    mul: f32,
    dec: usize,
    suffix: &'static str,
    /// A plus sign before positive numbers.
    plus: bool,
    /// What zero reads as, when it means «none» (no limit).
    zero: Option<&'static str>,
}

impl Num {
    /// `lo..=hi` in steps of `step`, shown with `dec` decimals and `suffix`.
    pub(crate) const fn new(lo: f32, hi: f32, step: f32, dec: usize, suffix: &'static str) -> Self {
        Self { lo, hi, step, mul: 1.0, dec, suffix, plus: false, zero: None }
    }

    /// A share `lo..=hi` (1 = 100 %), in percent.
    pub(crate) const fn pct(lo: f32, hi: f32, step: f32) -> Self {
        Self { lo, hi, step, mul: 100.0, dec: 0, suffix: " %", plus: false, zero: None }
    }

    pub(crate) const fn signed(mut self) -> Self {
        self.plus = true;
        self
    }

    /// Shown as the value times `mul` (seconds as milliseconds).
    pub(crate) const fn times(mut self, mul: f32) -> Self {
        self.mul = mul;
        self
    }

    /// Zero means «none» and reads as `label`.
    pub(crate) const fn zero(mut self, label: &'static str) -> Self {
        self.zero = Some(label);
        self
    }

    /// On a step, inside the range.
    pub(crate) fn snap(&self, v: f32) -> f32 {
        ((v / self.step).round() * self.step).clamp(self.lo, self.hi)
    }

    /// The value at `frac` of the track.
    fn at(&self, frac: f32) -> f32 {
        self.snap(self.lo + frac * (self.hi - self.lo))
    }

    fn frac(&self, v: f32) -> f32 {
        ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0)
    }

    pub(crate) fn fmt(&self, v: f32) -> String {
        let v = self.snap(v);
        if let Some(z) = self.zero
            && v.abs() < self.step * 0.5
        {
            return z.to_owned();
        }
        let mut t = format!("{:.*}", self.dec, v * self.mul);
        // No «-0».
        if t.starts_with('-') && t.chars().all(|c| matches!(c, '-' | '0' | '.')) {
            t.remove(0);
        }
        if self.plus && !t.starts_with('-') {
            t.insert(0, '+');
        }
        format!("{}{}", t.replace('.', ","), self.suffix)
    }

    /// What was typed into the field: the first number in it (a comma or a
    /// point for the decimals), in the field's unit; the zero label (or ∞)
    /// for none. Nothing for text without a number.
    pub(crate) fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim().to_lowercase();
        if let Some(z) = self.zero
            && (t == z || t.starts_with('∞') || t.starts_with("без"))
        {
            return Some(self.snap(0.0));
        }
        let n = first_number(&t)?;
        Some(self.snap(n / self.mul))
    }
}

/// The first number in `t` («+0,25 с» → 0.25).
fn first_number(t: &str) -> Option<f32> {
    let part = |c: &char| c.is_ascii_digit() || matches!(c, '-' | '+' | ',' | '.');
    let n: String = t.chars().skip_while(|c| !part(c)).take_while(|c| part(c)).collect();
    n.replace(',', ".").trim_start_matches('+').parse().ok()
}

/// A slider row: the slider on the controls' line and its exact value in
/// the field at the right. Returns a new value (dragged or typed).
pub(crate) fn slider(ui: &mut Ui, id: Id, value: f32, num: Num) -> Option<f32> {
    slider_with(ui, id, value, num, 0.0, |_| {})
}

/// [`slider`] with more controls after it, `extra_w` wide («Очистить»).
pub(crate) fn slider_with(ui: &mut Ui, id: Id, value: f32, num: Num, extra_w: f32, extra: impl FnOnce(&mut Ui)) -> Option<f32> {
    let text = num.fmt(value);
    let mut dragged = None;
    let mut typed = None;
    let mut field = |ui: &mut Ui| typed = value_field(ui, egui::Id::new(("set-value", id)), &text, VALUE_W);
    row_full(
        ui,
        id,
        None,
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let w = slider_width(ui, extra_w);
                let o = Slider::new(("set-slider", id), num.frac(value))
                    .width(w)
                    .track(8.0)
                    .value_label(move |v| num.fmt(num.at(v)))
                    .show(ui);
                dragged = o.committed.or(o.dragging).map(|v| num.at(v));
                extra(ui);
            });
        },
        Some(&mut field),
    );
    dragged.or_else(|| typed.and_then(|t| num.parse(&t)))
}

/// Every slider 240 wide, narrower only where the row has no room (all
/// the sliders of a column alike, their left edges on one line).
fn slider_width(ui: &Ui, extra_w: f32) -> f32 {
    (ui.available_width() - VALUE_W - VALUE_GAP - extra_w).clamp(160.0, 240.0)
}

/// The field of a slider's exact value: shows `shown`; while focused it
/// edits its own copy. Returns what was typed when it is left (Enter or a
/// click elsewhere), if it changed.
pub(crate) fn value_field(ui: &mut Ui, salt: egui::Id, shown: &str, width: f32) -> Option<String> {
    let s = Theme::get(ui.ctx()).scheme;
    let key = salt.with("typed");
    let edit = salt.with("edit");
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    // (as it read when the editing began, as it reads now)
    let mut buf: (String, String) = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_else(|| (shown.to_owned(), shown.to_owned()));
    let focused = ui.ctx().memory(|m| m.has_focus(edit));
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, s.surface_container_highest);
    if focused {
        painter.rect_stroke(rect, 8.0, Stroke::new(2.0, s.primary), egui::StrokeKind::Inside);
    }
    let line = TypeRole::BodyMedium.style().line_height;
    let inner = egui::Rect::from_center_size(rect.center(), vec2(width - 12.0, line));
    let resp = ui.put(
        inner,
        egui::TextEdit::singleline(&mut buf.1)
            .id(edit)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(TypeRole::BodyMedium.font_id())
            .text_color(s.on_surface)
            .desired_width(width - 12.0),
    );
    if resp.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(key, buf));
        return None;
    }
    ui.ctx().data_mut(|d| d.remove::<(String, String)>(key));
    (resp.lost_focus() && buf.1.trim() != buf.0.trim()).then_some(buf.1)
}

/// KiB/s slider mapping: quadratic, so the low end is fine-grained; the
/// far right end is no limit (the most there is).
const SPEED_MAX: f32 = 50_000.0;
/// The part of the track for limits; the rest, at the right, is no limit.
const SPEED_SPAN: f32 = 0.95;
/// The lowest limit: the left end is a limit too, never «no limit».
pub(super) const SPEED_MIN: u32 = 50;

/// Where `kbps` sits on the slider (0 = no limit: the right end).
pub(super) fn speed_frac(kbps: u32) -> f32 {
    if kbps == 0 { 1.0 } else { (kbps as f32 / SPEED_MAX).sqrt().clamp(0.0, 1.0) * SPEED_SPAN }
}

/// The limit at `v` on the slider (0 = no limit).
pub(super) fn speed_at(v: f32) -> u32 {
    if v >= (1.0 + SPEED_SPAN) / 2.0 {
        return 0;
    }
    let f = (v / SPEED_SPAN).clamp(0.0, 1.0);
    let raw = f * f * SPEED_MAX;
    (((raw / speed_step(raw)).round() * speed_step(raw)) as u32).max(SPEED_MIN)
}

fn speed_step(raw: f32) -> f32 {
    if raw < 1_000.0 {
        50.0
    } else if raw < 10_000.0 {
        250.0
    } else {
        1_000.0
    }
}

/// «без лимита», «500 КиБ/с», «1,5 МиБ/с».
pub(crate) fn fmt_speed(kbps: u32) -> String {
    match kbps {
        0 => "без лимита".to_owned(),
        k if k < 1024 => format!("{k} КиБ/с"),
        k => format!("{:.1} МиБ/с", k as f32 / 1024.0).replace('.', ","),
    }
}

/// A speed as its field shows it: «∞» for no limit.
fn speed_field_text(kbps: u32) -> String {
    if kbps == 0 { "∞".to_owned() } else { fmt_speed(kbps) }
}

/// A typed speed: KiB/s, or MiB/s with «м»/«m»; «∞» or «без…» for none.
pub(super) fn parse_speed(text: &str) -> Option<u32> {
    let t = text.trim().to_lowercase();
    if t.starts_with('∞') || t.starts_with("без") || t == "0" {
        return Some(0);
    }
    let n = first_number(&t)?;
    let kib = if t.contains('м') || t.contains('m') { n * 1024.0 } else { n };
    Some((kib.round().max(0.0) as u32).clamp(SPEED_MIN, SPEED_MAX as u32))
}

/// The field of a speed is wider: «1,5 МиБ/с».
const SPEED_W: f32 = 84.0;

/// A speed-limit row: the value under the pointer while dragging, and the
/// one let go of (or typed).
pub(crate) fn speed(ui: &mut Ui, id: Id, kbps: u32, enabled: bool) -> (Option<u32>, Option<u32>) {
    let text = speed_field_text(kbps);
    let mut moved = (None, None);
    let mut typed = None;
    let mut field = |ui: &mut Ui| {
        typed = ui.add_enabled_ui(enabled, |ui| value_field(ui, egui::Id::new(("set-value", id)), &text, SPEED_W)).inner;
    };
    row_full(
        ui,
        id,
        None,
        |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let w = (ui.available_width() - SPEED_W - VALUE_GAP).clamp(160.0, 240.0);
                let o = Slider::new(("set-slider", id), speed_frac(kbps))
                    .width(w)
                    .track(8.0)
                    .value_label(move |v| fmt_speed(speed_at(v)))
                    .show(ui);
                moved = (o.dragging.map(speed_at), o.committed.map(speed_at));
            });
        },
        Some(&mut field),
    );
    let typed = typed.and_then(|t| parse_speed(&t)).filter(|v| *v != kbps);
    (moved.0, moved.1.or(typed))
}

/// A list's title as a group's, with its ⓘ.
pub(crate) fn list_title(ui: &mut Ui, id: Id) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(TypeRole::TitleSmall.style().rich(id.label(), s.primary));
        settings_help::help_button(ui, id);
    });
    ui.add_space(4.0);
}

/// One mark of a list: its name and what takes it back («Снять»,
/// «Вернуть»).
pub(crate) fn mark_row(ui: &mut Ui, salt: egui::Id, title: &str, sub: Option<&str>, label: &str, mut take_back: impl FnMut()) {
    let mut item = crate::components::ListItem::new(title).id_salt(salt);
    if let Some(sub) = sub {
        item = item.supporting(sub);
    }
    let mut clicked = false;
    item.trailing(|ui| {
        clicked = ui.add(Button::new(label).style(Style::Text).id_salt(salt.with("take-back"))).clicked();
    })
    .show(ui);
    if clicked {
        take_back();
    }
}

/// A progress bar under a switch while background work goes on (no words).
pub(crate) fn switch_with_progress(ui: &mut Ui, id: Id, on: &mut bool, progress: Option<f32>, note: Option<Note<'_>>) {
    row_note(ui, id, note, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add(Switch::new(on).id_salt(("set-switch", id)));
            if let Some(f) = progress {
                crate::components::wavy_progress(ui, ("set-progress", id), f, 96.0);
            }
        });
    });
}

pub(crate) fn open_folder(path: &std::path::Path) -> std::io::Result<()> {
    // A test must never launch a program on the user's desktop.
    if cfg!(test) {
        panic!("a test tried to launch an external program");
    }
    let _ = std::fs::create_dir_all(path);
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut child = std::process::Command::new(opener).arg(path).spawn()?;
    // Reaped in the background, or the finished opener stays a zombie.
    #[cfg(unix)]
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || {
        let _ = child.wait();
    });
    #[cfg(not(unix))]
    drop(child);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// «Без лимита» is the far right end, the most there is; the left end
    /// is the lowest limit, and a limit stays where it was set.
    #[test]
    fn no_limit_is_the_right_end_of_a_speed_slider() {
        assert_eq!(speed_frac(0), 1.0);
        assert_eq!(speed_at(1.0), 0);
        assert_eq!(fmt_speed(speed_at(1.0)), "без лимита");
        assert_eq!(speed_at(0.0), SPEED_MIN, "the left end is a limit");
        let mut last = 0;
        for i in 0..=90 {
            let v = speed_at(i as f32 / 100.0);
            assert!(v > 0 && v >= last, "rising to the right: {v} after {last}");
            last = v;
        }
        for kbps in [50, 500, 2_000, 10_000, 50_000] {
            assert_eq!(speed_at(speed_frac(kbps)), kbps, "{kbps} stays");
            assert!(speed_frac(kbps) < speed_frac(0));
        }
    }

    /// The value fields read as Russian writes numbers, and what is typed
    /// in them comes back on the slider's steps and inside its range.
    #[test]
    fn value_fields_read_and_take_numbers() {
        let lead = Num::new(-0.5, 0.5, 0.05, 2, " с").signed();
        assert_eq!(lead.fmt(0.2), "+0,20 с");
        assert_eq!(lead.fmt(0.0), "+0,00 с");
        assert_eq!(lead.fmt(-0.25), "-0,25 с");
        let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
        assert!(near(lead.parse("+0,25 с"), 0.25));
        assert!(near(lead.parse("-0.1"), -0.1));
        assert_eq!(lead.parse("9"), Some(0.5), "clamped to the range");
        assert_eq!(lead.parse("быстро"), None);
        let pct = Num::pct(0.8, 1.4, 0.05);
        assert_eq!(pct.fmt(0.9), "90 %");
        assert!((pct.parse("115 %").unwrap() - 1.15).abs() < 1e-4);
        let ratio = Num::new(0.0, 5.0, 0.5, 1, "").zero("∞");
        assert_eq!(ratio.fmt(0.0), "∞");
        assert_eq!(ratio.fmt(3.5), "3,5");
        assert_eq!(ratio.parse("∞"), Some(0.0));
        assert_eq!(ratio.parse("2,2"), Some(2.0));
        let wait = Num::new(0.0, 0.4, 0.02, 0, " мс").times(1000.0);
        assert_eq!(wait.fmt(0.22), "220 мс");
        assert!((wait.parse("100").unwrap() - 0.1).abs() < 1e-4);
        assert_eq!(parse_speed("∞"), Some(0));
        assert_eq!(parse_speed("500"), Some(500));
        assert_eq!(parse_speed("1,5 МиБ/с"), Some(1536));
        assert_eq!(speed_field_text(0), "∞");
    }

    #[test]
    fn levels_show_by_mode() {
        let simple = Mode { deep: false, dev: true, opacity: 1.0 };
        let deep = Mode { deep: true, dev: false, opacity: 1.0 };
        let own = Mode { deep: true, dev: true, opacity: 1.0 };
        assert!(level_shown(simple, Level::Simple) && !level_shown(simple, Level::Deep) && !level_shown(simple, Level::Dev));
        assert!(level_shown(deep, Level::Deep) && !level_shown(deep, Level::Dev));
        assert!(level_shown(own, Level::Dev));
        assert!(label_column(480.0) < 480.0 - 160.0 - VALUE_W, "a slider and its value fit beside the name at 480");
    }
}
