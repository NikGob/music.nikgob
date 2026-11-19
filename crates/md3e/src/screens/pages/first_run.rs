//! The first run: a page of its own over the whole window (no rail, search
//! or player), shown while `Prefs::first_run` is on. A few short steps —
//! where the music comes from, whether it is kept on the computer, the
//! sign-ins, the look of the lyrics, a few settings — each skippable, with
//! «Назад» and ✕ (which ends it) everywhere. What the choices write is
//! [`nikgob_view::first_run`]'s; the sign-ins are Настройки → «Сервисы»'s
//! own parts and commands.
//!
//! Steps change on the default spatial spring: the new one slides in from
//! the side it comes from while its parts fade in one after another.

use egui::{Align2, Color32, CursorIcon, Id, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use nikgob_core::i18n;
use nikgob_view::first_run::{self as fr, Choices, Keep, Source};
use nikgob_view::presets::Area;
use nikgob_view::{Account, AccountView, Command, LoginInput, PrefPatch, Prefs, Service, ThemeMode, ViewState};

use super::settings_help::SettingId;
use super::{PageState, cards, connected, logos, settings};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, IconButton, LoadingIndicator, Size, Style, Switch, TextField, anim},
    tokens::shape,
};

/// Where Spotify's export of the whole listening history is ordered.
const SPOTIFY_EXPORT: &str = "https://www.spotify.com/account/privacy/";
/// Google Takeout: YouTube Music's history.
const TAKEOUT: &str = "https://takeout.google.com/";

/// A step of the first run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Step {
    #[default]
    Sources,
    Keep,
    Connect,
    Lyrics,
    More,
}

impl Step {
    /// The steps the choices so far lead through.
    pub(crate) fn path(c: &Choices) -> Vec<Self> {
        let mut v = vec![Self::Sources];
        if c.picks.asks_keep() {
            v.push(Self::Keep);
        }
        if c.picks.connects() {
            v.push(Self::Connect);
        }
        v.extend([Self::Lyrics, Self::More]);
        v
    }

    fn title(self) -> &'static str {
        let f = i18n::first_run();
        match self {
            Self::Sources => f.step_sources,
            Self::Keep => f.step_keep,
            Self::Connect => f.step_connect,
            Self::Lyrics => f.step_lyrics,
            Self::More => f.step_more,
        }
    }

    /// The step a screenshot names (`first_run:connect`).
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "" | "sources" => Self::Sources,
            "keep" => Self::Keep,
            "connect" => Self::Connect,
            "lyrics" => Self::Lyrics,
            "more" => Self::More,
            _ => return None,
        })
    }

    /// How wide the step's column is at most.
    fn column(self) -> f32 {
        match self {
            // Five tiles of 164 and their gaps; four lyrics cards of 208.
            Self::Sources => 868.0,
            Self::Lyrics => 880.0,
            // Two tiles of 260 and the gap.
            Self::Keep => 532.0,
            Self::Connect | Self::More => 600.0,
        }
    }
}

/// What the first run keeps between frames.
#[derive(Default)]
pub(crate) struct State {
    step: Step,
    choices: Choices,
    /// The prefs it started from: each choice is written from them.
    start: Option<Prefs>,
    /// +1: the last move went forward; -1: back.
    dir: f32,
    /// When the step on screen came.
    since: Option<f64>,
    /// «Перенести историю» is open.
    history: bool,
    /// Spotify's steps to its own key are open.
    spotify_steps: bool,
    /// The folder picked on «Подключи» (its name).
    folder: Option<String>,
    /// «Как к тебе обращаться» as typed.
    name: String,
    /// It was closed: hidden until the prefs say so too.
    closing: bool,
}

/// The first run is on screen.
pub(crate) fn shown(state: &ViewState, pages: &mut PageState) -> bool {
    let st = &mut pages.first_run;
    if !state.prefs.first_run {
        st.closing = false;
        return false;
    }
    !st.closing
}

/// A screenshot opens it at `step` with `picks` («spotify+folder») chosen.
pub(crate) fn jump(state: &ViewState, pages: &mut PageState, step: Step, picks: &str, out: &mut Vec<Command>) {
    let st = &mut pages.first_run;
    *st = State::default();
    let start = state.prefs.clone();
    for name in picks.split('+') {
        let pick = match name.trim() {
            "spotify" => Source::Spotify,
            "youtube" => Source::YouTube,
            "soundcloud" => Source::SoundCloud,
            "folder" => Source::Folder,
            "local" => Source::LocalOnly,
            _ => continue,
        };
        st.choices.picks.toggle(pick);
    }
    st.start = Some(start.clone());
    st.step = if Step::path(&st.choices).contains(&step) { step } else { Step::Sources };
    st.dir = 1.0;
    st.name = start.user_name.clone();
    if !state.prefs.first_run {
        out.push(Command::SetPref(PrefPatch::FirstRun(true)));
    }
    write_choices(st, state, out);
}

/// Draws the first run over `full`.
pub(crate) fn show(ui: &mut Ui, full: Rect, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let now = anim::now(&ctx);
    {
        let st = &mut pages.first_run;
        if st.start.is_none() {
            st.start = Some(state.prefs.clone());
            st.name = state.prefs.user_name.clone();
            st.dir = 1.0;
            // On screen: a start after the app quit midway shows it again.
            if !state.prefs.first_run_started {
                out.push(Command::SetPref(PrefPatch::FirstRunStarted));
            }
        }
        if st.since.is_none() {
            st.since = Some(now);
        }
        if let Some(path) = ctx.data_mut(|d| d.remove_temp::<String>(folder_key())) {
            st.folder = Some(path);
        }
        if !Step::path(&st.choices).contains(&st.step) {
            st.step = Step::Sources;
        }
    }
    let narrow = full.width() < 600.0;
    let margin = if narrow { 16.0 } else { 24.0 };

    // The top: the steps' indicator in the middle, ✕ at the right.
    let path = Step::path(&pages.first_run.choices);
    let at = path.iter().position(|s| *s == pages.first_run.step).unwrap_or(0);
    let top = Rect::from_min_size(full.min + vec2(margin, 8.0), vec2(full.width() - 2.0 * margin, 56.0));
    indicator(ui, top.center(), path.len(), at);
    let close = Rect::from_center_size(pos2(top.right() - 20.0, top.center().y), Vec2::splat(40.0));
    if ui.put(close, IconButton::new(Icon::Close).tooltip(i18n::common().close).id_salt("first-run-close")).clicked() {
        finish(pages, state, false, out);
        return;
    }

    let body = Rect::from_min_max(pos2(full.left(), top.bottom()), full.max);
    let mut bui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("first-run-body"));
    bui.set_clip_rect(body);
    let step = pages.first_run.step;
    let height_key = Id::new(("first-run-height", step));
    let last_h: f32 = ctx.data(|d| d.get_temp(height_key)).unwrap_or(body.height());
    let mut act = None;
    egui::ScrollArea::vertical()
        .id_salt(("first-run-scroll", step))
        .auto_shrink([false, false])
        .show(&mut bui, |ui| {
            let view_h = body.height();
            let pad = ((view_h - last_h) / 2.0 - 16.0).max(8.0);
            ui.add_space(pad);
            let col_w = step.column().min(body.width() - 2.0 * margin);
            let left = body.center().x - col_w / 2.0;
            let since = pages.first_run.since.unwrap_or(now);
            let dir = pages.first_run.dir;
            let slide = Slide { since, dir };
            let (dx, _) = slide.at(&ctx, 0);
            let rect = Rect::from_min_size(pos2(left + dx, ui.cursor().top()), vec2(col_w, 10_000.0));
            let mut col = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("first-run-col", step)));
            col.spacing_mut().item_spacing = vec2(0.0, 0.0);
            act = column(&mut col, step, at, path.len(), &slide, state, pages, out);
            let h = col.min_rect().height();
            ctx.data_mut(|d| d.insert_temp(height_key, h));
            ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        });

    // Esc goes back a step (the first one keeps it).
    if at > 0 && act.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        act = Some(Act::Back);
    }
    match act {
        Some(Act::Back) => go(pages, &path, at.saturating_sub(1), -1.0, now),
        Some(Act::Next) if at + 1 < path.len() => go(pages, &path, at + 1, 1.0, now),
        Some(Act::Next | Act::Done) => finish(pages, state, true, out),
        None => {}
    }
}

fn folder_key() -> Id {
    Id::new("first-run-folder-picked")
}

/// What the step's buttons asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Back,
    Next,
    Done,
}

fn go(pages: &mut PageState, path: &[Step], to: usize, dir: f32, now: f64) {
    let st = &mut pages.first_run;
    if let Some(s) = path.get(to) {
        st.step = *s;
        st.dir = dir;
        st.since = Some(now);
    }
}

/// Ends the first run: finished (`done`: the name typed is kept, Главная
/// asks for it no more) or closed with ✕ (what was chosen stays).
fn finish(pages: &mut PageState, state: &ViewState, done: bool, out: &mut Vec<Command>) {
    let st = &mut pages.first_run;
    if done {
        let name = nikgob_view::user_name(&st.name);
        if name.is_empty() {
            out.push(Command::SetPref(PrefPatch::NamePrompt(false)));
        } else if name != state.prefs.user_name {
            out.push(Command::SetPref(PrefPatch::UserName(name)));
            pages.name_synced = false;
        }
    }
    out.push(Command::SetPref(PrefPatch::FirstRun(false)));
    pages.first_run = State { closing: true, ..State::default() };
}

/// Sends what the choices write, from the prefs the first run started with.
fn write_choices(st: &State, state: &ViewState, out: &mut Vec<Command>) {
    let Some(start) = st.start.as_ref() else { return };
    let mut after = state.prefs.clone();
    fr::apply(&mut after, start, &st.choices);
    if after != state.prefs {
        out.push(Command::SetPref(PrefPatch::Edit { before: Box::new(state.prefs.clone()), after: Box::new(after) }));
    }
}

/// The parts of a step coming in: the column slides from the side it
/// comes from on the default spatial spring, each part fades in a little
/// after the one before.
struct Slide {
    since: f64,
    dir: f32,
}

impl Slide {
    /// Part `n`'s offset and opacity now.
    fn at(&self, ctx: &egui::Context, n: usize) -> (f32, f32) {
        let theme = Theme::get(ctx);
        let t = (anim::now(ctx) - self.since) as f32 - 0.045 * n.min(6) as f32;
        if t >= 1.4 {
            return (0.0, 1.0);
        }
        ctx.request_repaint();
        if t <= 0.0 {
            return (if theme.reduced_motion { 0.0 } else { 48.0 * self.dir }, 0.0);
        }
        let alpha = (1.0 - theme.motion.default_effects.solve(1.0, 0.0, t).0).clamp(0.0, 1.0);
        let dx = if theme.reduced_motion { 0.0 } else { 48.0 * self.dir * theme.motion.default_spatial.solve(1.0, 0.0, t).0 };
        (dx, alpha)
    }

    /// Draws part `n` (faded, and moved by what the column has not moved).
    fn part<R>(&self, ui: &mut Ui, n: usize, add: impl FnOnce(&mut Ui) -> R) -> R {
        let ctx = ui.ctx().clone();
        let (dx, alpha) = self.at(&ctx, n);
        let (dx0, _) = self.at(&ctx, 0);
        ui.scope(|ui| {
            if alpha < 1.0 {
                ui.multiply_opacity(alpha);
            }
            cards::moved(ui, vec2(dx - dx0, 0.0), add)
        })
        .inner
    }
}

/// The step's column: its title, its body, its buttons.
#[allow(clippy::too_many_arguments)]
fn column(ui: &mut Ui, step: Step, at: usize, steps: usize, slide: &Slide, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Act> {
    let s = Theme::get(ui.ctx()).scheme;
    let w = ui.available_width();
    slide.part(ui, 0, |ui| {
        if step == Step::Sources {
            super::brand::mark(ui, 56.0);
            ui.add_space(16.0);
        }
        let mut job = TypeRole::HeadlineMedium.style().job(step.title(), s.on_surface);
        job.halign = egui::Align::Center;
        job.wrap.max_width = w;
        let g = crate::typography::layout(ui.painter(), job);
        let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x, r.top()), g, s.on_surface);
    });
    ui.add_space(32.0);
    let chosen = match step {
        Step::Sources => sources(ui, slide, state, pages, out),
        Step::Keep => keep(ui, slide, state, pages, out),
        Step::Connect => {
            connect(ui, slide, state, pages, out);
            true
        }
        Step::Lyrics => {
            lyrics(ui, slide, state, out);
            true
        }
        Step::More => {
            more(ui, slide, state, pages, out);
            true
        }
    };
    ui.add_space(32.0);
    slide.part(ui, 7, |ui| buttons(ui, at, steps, step, chosen))
}

/// «Назад» at the left; at the right «Далее» once something is chosen,
/// «Пропустить» while nothing is, «Готово» on the last step — one line.
fn buttons(ui: &mut Ui, at: usize, steps: usize, step: Step, chosen: bool) -> Option<Act> {
    let f = i18n::first_run();
    let c = i18n::common();
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, Size::M.height()), Sense::hover());
    let mut act = None;
    let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt("first-run-buttons"));
    right.spacing_mut().item_spacing.x = 8.0;
    let last = at + 1 == steps;
    if last {
        if right.add(Button::new(c.done).style(Style::Filled).size(Size::M).id_salt("first-run-done")).clicked() {
            act = Some(Act::Done);
        }
    } else {
        let (text, style) = if chosen { (f.next, Style::Filled) } else { (c.skip, Style::Tonal) };
        if right.add(Button::new(text).style(style).size(Size::M).id_salt(("first-run-next", step))).clicked() {
            act = Some(Act::Next);
        }
    }
    if at > 0 {
        // The text button's own padding hangs out: its arrow stands on the
        // column's edge.
        let at = row.translate(vec2(-Size::M.side_padding(), 0.0));
        let mut left = ui.new_child(UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("first-run-back"));
        if left.add(Button::new(c.back).icon(Icon::ArrowBack).style(Style::Text).size(Size::M).id_salt("first-run-back-button")).clicked() {
            act = Some(Act::Back);
        }
    }
    act
}

/// The steps as dots, the one on screen a pill (its width on the fast
/// spatial spring).
fn indicator(ui: &Ui, center: egui::Pos2, n: usize, at: usize) {
    let ctx = ui.ctx();
    let theme = Theme::get(ctx);
    let s = theme.scheme;
    const DOT: f32 = 8.0;
    const PILL: f32 = 28.0;
    const GAP: f32 = 8.0;
    let widths: Vec<f32> = (0..n)
        .map(|i| anim::f32(ctx, Id::new(("first-run-dot", i)), if i == at { PILL } else { DOT }, theme.motion.fast_spatial))
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + GAP * n.saturating_sub(1) as f32;
    let mut x = center.x - total / 2.0;
    for (i, w) in widths.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, center.y - DOT / 2.0), vec2(w.max(DOT), DOT));
        let c = if i == at {
            s.primary
        } else if i < at {
            s.on_surface_variant
        } else {
            s.outline_variant
        };
        ui.painter().rect_filled(r, DOT / 2.0, c);
        x += w.max(DOT) + GAP;
    }
}

/// What a tile shows over its name.
#[derive(Clone, Copy, Debug)]
pub(super) enum Art {
    Logo(Service),
    Icon(Icon),
}

/// A choice tile: the mark or icon over the name; picked, its container
/// turns secondary, its corners round further and a check comes in at its
/// corner (all on springs). True when clicked.
pub(super) fn tile(ui: &mut Ui, rect: Rect, id: Id, art: Art, label: &str, picked: bool) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let down = resp.is_pointer_button_down_on();
    let press = anim::f32(&ctx, id.with("press"), if down { 0.96 } else { 1.0 }, theme.motion.fast_spatial);
    let radius = anim::f32(&ctx, id.with("radius"), if picked { shape::EXTRA_LARGE } else { shape::LARGE }, theme.motion.fast_spatial);
    let on = anim::fraction(&ctx, id.with("on"), if picked { 1.0 } else { 0.0 }, theme.motion.fast_effects);
    let badge = anim::f32(&ctx, id.with("badge"), if picked { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    let card = Rect::from_center_size(rect.center(), rect.size() * press);
    let p = ui.painter();
    p.rect_filled(card, radius, mix(s.surface_container_high, s.secondary_container, on));
    let content = mix(s.on_surface, s.on_secondary_container, on);
    let layer = if down {
        0.10
    } else if resp.hovered() {
        0.08
    } else {
        0.0
    };
    let layer = anim::fraction(&ctx, id.with("layer"), layer, theme.motion.fast_effects);
    if layer > 0.001 {
        p.rect_filled(card, radius, content.gamma_multiply(layer));
    }
    let side = (card.height() * 0.34).clamp(28.0, 48.0);
    let label_h = TypeRole::TitleSmall.style().line_height;
    let gap = 12.0;
    let top = card.center().y - (side + gap + label_h) / 2.0;
    let art_rect = Rect::from_center_size(pos2(card.center().x, top + side / 2.0), Vec2::splat(side));
    match art {
        Art::Logo(service) => logos::paint(ui, art_rect, service, 1.0),
        Art::Icon(icon) => {
            icon.widget(picked, mix(s.on_surface_variant, s.on_secondary_container, on), side).paint_at(ui, art_rect);
        }
    }
    let mut job = TypeRole::TitleSmall.style().job(label, content);
    job.wrap = egui::text::TextWrapping::truncate_at_width(card.width() - 16.0);
    let g = crate::typography::layout(ui.painter(), job);
    ui.painter().galley(pos2(card.center().x - g.size().x / 2.0, top + side + gap), g, content);
    if badge > 0.01 {
        let c = pos2(card.right() - 20.0, card.top() + 20.0);
        let r = 12.0 * badge.max(0.0);
        ui.painter().circle_filled(c, r, s.primary.gamma_multiply(on));
        let icon = Rect::from_center_size(c, Vec2::splat(16.0 * badge.clamp(0.0, 1.2)));
        Icon::Check.widget(false, s.on_primary.gamma_multiply(on), icon.width()).paint_at(ui, icon);
    }
    resp.clicked()
}

/// Lays `n` tiles in rows of as many as fit at most `max_w` wide (and at
/// least `min_w`), centred; the rects in order.
fn grid(ui: &mut Ui, n: usize, min_w: f32, max_w: f32, h: f32, gap: f32) -> Vec<Rect> {
    let w = ui.available_width();
    let per = (((w + gap) / (min_w + gap)).floor() as usize).clamp(1, n.max(1));
    // Even rows: 5 on a narrow page go 3 + 2, not 4 + 1.
    let rows = n.div_ceil(per);
    let per = n.div_ceil(rows);
    let tile_w = ((w - gap * (per - 1) as f32) / per as f32).min(max_w);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (h + gap) - gap), Sense::hover());
    (0..n)
        .map(|i| {
            let (row, k) = (i / per, i % per);
            let in_row = (n - row * per).min(per);
            let row_w = in_row as f32 * tile_w + (in_row - 1) as f32 * gap;
            let x = area.center().x - row_w / 2.0 + k as f32 * (tile_w + gap);
            Rect::from_min_size(pos2(x, area.top() + row as f32 * (h + gap)), vec2(tile_w, h))
        })
        .collect()
}

/// Step 1: the sources, several at once. True when one is picked.
fn sources(ui: &mut Ui, slide: &Slide, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> bool {
    let st = &mut pages.first_run;
    let rects = slide.part(ui, 1, |ui| grid(ui, Source::ALL.len(), 136.0, 164.0, 136.0, 12.0));
    let mut changed = false;
    for (i, (src, r)) in Source::ALL.iter().zip(&rects).enumerate() {
        let art = match src.service() {
            Some(service) => Art::Logo(service),
            None if *src == Source::Folder => Art::Icon(Icon::Folder),
            None => Art::Icon(Icon::DownloadDone),
        };
        let picked = st.choices.picks.has(*src);
        if slide.part(ui, 2 + i.min(4), |ui| tile(ui, *r, Id::new(("first-run-source", *src)), art, src.label(), picked)) {
            st.choices.picks.toggle(*src);
            changed = true;
        }
    }
    if changed {
        write_choices(st, state, out);
    }
    !st.choices.picks.is_empty()
}

/// Step 2: keep the music on the computer, or only listen. True when
/// answered.
fn keep(ui: &mut Ui, slide: &Slide, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> bool {
    let st = &mut pages.first_run;
    let rects = slide.part(ui, 1, |ui| grid(ui, 2, 200.0, 260.0, 152.0, 12.0));
    let f = i18n::first_run();
    let options = [(Keep::Files, Icon::Download, f.keep_yes), (Keep::Listen, Icon::GraphicEq, f.keep_no)];
    for (i, ((k, icon, label), r)) in options.iter().zip(&rects).enumerate() {
        let picked = st.choices.keep == Some(*k);
        if slide.part(ui, 2 + i, |ui| tile(ui, *r, Id::new(("first-run-keep", *k)), Art::Icon(*icon), label, picked)) && !picked {
            st.choices.keep = Some(*k);
            write_choices(st, state, out);
        }
    }
    st.choices.keep.is_some()
}

/// Step 3: what was picked, one card each, with the sign-in of
/// Настройки → «Сервисы» or the folder picker.
fn connect(ui: &mut Ui, slide: &Slide, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let picks: Vec<Source> = pages.first_run.choices.picks.iter().filter(|s| *s != Source::LocalOnly).collect();
    for (i, src) in picks.iter().enumerate() {
        if i > 0 {
            ui.add_space(12.0);
        }
        slide.part(ui, 1 + i, |ui| connect_card(ui, *src, state, pages, out));
    }
}

fn connect_card(ui: &mut Ui, src: Source, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    egui::Frame::new()
        .fill(s.surface_container_low)
        .corner_radius(egui::CornerRadius::same(shape::LARGE as u8))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
            match src.service() {
                Some(service) => service_card(ui, service, state, pages, out),
                None => folder_card(ui, pages, out),
            }
        });
}

/// The card's first line: the mark (or icon), the name, and at the right
/// `control`.
/// On a card narrower than this the control goes under the name.
const CARD_NARROW: f32 = 400.0;

fn card_head(ui: &mut Ui, art: Art, name: &str, control: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    let w = ui.available_width();
    let narrow = w < CARD_NARROW;
    let (row, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
    let mark = Rect::from_min_size(pos2(row.left(), row.center().y - 20.0), Vec2::splat(40.0));
    match art {
        Art::Logo(service) => logos::paint(ui, mark, service, 1.0),
        Art::Icon(icon) => {
            icon.widget(false, s.on_surface_variant, 32.0).paint_at(ui, Rect::from_center_size(mark.center(), Vec2::splat(32.0)));
        }
    }
    // The control first (right to left from the edge), then the name in
    // the room it leaves.
    let place = if narrow {
        let (under, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
        Rect::from_min_max(pos2(mark.right() + 16.0, under.top()), under.max)
    } else {
        row
    };
    let layout = if narrow { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::right_to_left(egui::Align::Center) };
    let mut c = ui.new_child(UiBuilder::new().max_rect(place).layout(layout).id_salt(("first-run-card", name)));
    c.spacing_mut().item_spacing.x = 8.0;
    control(&mut c);
    let taken = if narrow || c.min_rect().width() < 1.0 { row.right() } else { c.min_rect().left() - 12.0 };
    let mut job = TypeRole::TitleMedium.style().job(name, s.on_surface);
    job.wrap = egui::text::TextWrapping::truncate_at_width((taken - mark.right() - 16.0).max(24.0));
    let g = crate::typography::layout(ui.painter(), job);
    ui.painter().galley(pos2(mark.right() + 16.0, row.center().y - g.size().y / 2.0), g, s.on_surface);
}

/// Who is signed in (or the folder picked), with a check before it: the
/// card's value, in the head's right-to-left row.
fn signed_in(ui: &mut Ui, who: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    let mut job = TypeRole::BodyMedium.style().job(who, s.on_surface_variant);
    job.wrap = egui::text::TextWrapping::truncate_at_width(200.0);
    let check = |ui: &mut Ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
        Icon::Check.widget(false, s.primary, 20.0).paint_at(ui, r);
    };
    // The check before the name, whichever way the row runs.
    if ui.layout().main_dir() == egui::Direction::RightToLeft {
        ui.label(job);
        check(ui);
    } else {
        check(ui);
        ui.label(job);
    }
}

fn service_card(ui: &mut Ui, service: Service, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let tabs = i18n::settings_tabs();
    let acc = state.account(service);
    let registered = acc.is_some_and(|a| a.registered);
    let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
    let name = acc.and_then(|a| match &a.account {
        Account::LoggedIn { name, .. } => Some(name.clone()),
        _ => None,
    });
    let label = match service {
        Service::Spotify => "Spotify",
        Service::Youtube => "YouTube Music",
        Service::Soundcloud => "SoundCloud",
        Service::Deezer => "Deezer",
    };
    let enabled = state.prefs.service_enabled(service);
    let personal = state.prefs.spotify_personal_app;
    let st = &mut pages.first_run;
    let mut steps_toggled = false;
    card_head(ui, Art::Logo(service), label, |ui| {
        if let Some(who) = &name {
            signed_in(ui, who);
        } else if busy {
            ui.add(LoadingIndicator::new().size(32.0));
        } else if !registered {
            ui.label(TypeRole::BodyMedium.style().rich(tabs.not_in_this_build, s.error));
        } else {
            let (text, salt) = match service {
                Service::Spotify if !personal => (tabs.connect, "first-run-sp-connect"),
                Service::Spotify => (tabs.login_spotify, "first-run-sp-login"),
                Service::Youtube => (tabs.login_google, "first-run-yt-login"),
                _ => (tabs.login, "first-run-sc-login"),
            };
            let b = Button::new(text).icon(Icon::Login).style(Style::Tonal).enabled(enabled).selected(service == Service::Spotify && st.spotify_steps).id_salt(salt);
            if ui.add(b).clicked() {
                if service == Service::Spotify && !personal {
                    steps_toggled = true;
                } else {
                    out.push(Command::StreamLogin { service, input: LoginInput::Browser });
                }
            }
        }
    });
    if steps_toggled {
        st.spotify_steps = !st.spotify_steps;
    }
    let steps = service == Service::Spotify && st.spotify_steps && name.is_none() && !personal;
    if let Some(a) = acc {
        account_parts(ui, a, out);
    }
    if name.is_none() && registered && matches!(service, Service::Youtube | Service::Soundcloud) {
        manual(ui, service, &state.prefs, pages, enabled && !busy, out);
    }
    if steps {
        ui.add_space(8.0);
        settings::spotify_steps(ui, &state.prefs, pages, registered && !busy, out);
    }
    // Signed in means Premium: the sound is one more consent away.
    if service == Service::Spotify && name.is_some() && acc.is_some_and(|a| !a.audio) {
        ui.add_space(4.0);
        if ui.add(Button::new(tabs.enable_sound).icon(Icon::VolumeUp).style(Style::Tonal).enabled(!busy).id_salt("first-run-sp-sound")).clicked() {
            out.push(Command::StreamLogin { service, input: LoginInput::Playback });
        }
        ui.add_space(4.0);
    }
}

/// The way in without the sign-in window, as in Настройки → «Сервисы»
/// (where the system has no web view for it, as Linux without WebKitGTK,
/// or it did not help): «Вставить cookies…» / «Вставить токен…» under the
/// name, opening the same fields.
fn manual(ui: &mut Ui, service: Service, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    let youtube = service == Service::Youtube;
    let open = if youtube { pages.yt_form } else { pages.sc_form };
    let st = i18n::settings_tabs();
    let (label, salt) = if youtube { (st.paste_cookies, "first-run-yt-cookies") } else { (st.paste_token, "first-run-sc-token") };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), Size::S.height()), Sense::hover());
    // The text under the name (the mark's 40 and its 16), the button's
    // own padding out of the way.
    let at = Rect::from_min_max(pos2(row.left() + 56.0 - Size::S.side_padding(), row.top()), row.max);
    let mut c = ui.new_child(UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt(salt));
    if c.add(Button::new(label).style(Style::Text).size(Size::S).selected(open).id_salt(salt)).clicked() {
        if youtube {
            pages.yt_form = !open;
        } else {
            pages.sc_form = !open;
        }
    }
    if !open {
        return;
    }
    ui.add_space(8.0);
    if youtube {
        settings::cookies_fields(ui, p, pages, can, out);
    } else {
        let f = i18n::first_run();
        settings::step(ui, 1, f.sc_step1);
        settings::step(ui, 2, f.sc_step2);
        ui.add_space(4.0);
        settings::token_fields(ui, pages, can, false, out);
    }
    ui.add_space(4.0);
}

/// The running sign-in's prompt, its error and a pause, as Настройки
/// shows them.
fn account_parts(ui: &mut Ui, a: &AccountView, out: &mut Vec<Command>) {
    let shows = matches!(a.account, Account::LoggingIn(Some(_)) | Account::Failed(_)) || a.error.is_some() || a.paused_until_hm.is_some();
    if shows {
        ui.add_space(8.0);
        settings::login_prompt(ui, a, out);
        settings::login_error(ui, a);
        settings::paused_line(ui, a);
    }
}

fn folder_card(ui: &mut Ui, pages: &mut PageState, out: &mut Vec<Command>) {
    let f = i18n::first_run();
    let picked = pages.first_run.folder.clone();
    card_head(ui, Art::Icon(Icon::Folder), Source::Folder.label(), |ui| {
        let text = if picked.is_some() { f.other_folder } else { f.pick_folder };
        let style = if picked.is_some() { Style::Text } else { Style::Tonal };
        if ui.add(Button::new(text).style(style).id_salt("first-run-folder")).clicked() {
            let ctx = ui.ctx().clone();
            super::dialogs::folder(
                |d| d.set_title(f.music_folder_title),
                move |path| {
                    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    ctx.data_mut(|d| d.insert_temp(folder_key(), name));
                    ctx.request_repaint();
                    Command::ImportFolder(path)
                },
                out,
            );
        }
        if let Some(name) = &picked {
            signed_in(ui, name);
        }
    });
}

/// Step 4: the four looks of «Вид текста», live; a click picks the preset
/// (the same patch as Настройки → «Тексты»).
fn lyrics(ui: &mut Ui, slide: &Slide, state: &ViewState, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let names = Area::Lyrics.names();
    let current = Area::Lyrics.current(&state.prefs);
    let label_h = TypeRole::LabelLarge.style().line_height + 8.0;
    // Four in a row on a wide page, two on a narrow one.
    let w = ui.available_width();
    let per = if w >= 640.0 { 4 } else { 2 };
    let card_w = ((w - 16.0 * (per - 1) as f32) / per as f32).min(208.0);
    let card_h = (card_w * 0.62).round();
    let rects = slide.part(ui, 1, |ui| grid(ui, names.len(), card_w, card_w, card_h + label_h, 16.0));
    for (i, (name, r)) in names.iter().zip(&rects).enumerate() {
        slide.part(ui, 2 + i, |ui| {
            let id = Id::new(("first-run-lyrics", i));
            let resp = ui.interact(*r, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let card = Rect::from_min_size(r.min, vec2(r.width(), card_h));
            let picked = current == Some(i);
            let radius = anim::f32(&ctx, id.with("radius"), if picked { shape::EXTRA_LARGE } else { shape::LARGE }, theme.motion.fast_spatial);
            ui.painter().rect_filled(card, radius, s.surface_container_high);
            // The picked look and the one under the pointer play; the rest
            // stand half sung.
            let moving = (picked || resp.hovered()) && !theme.reduced_motion;
            // At the largest text size the sample has room for.
            let shown = Prefs { lyrics_scale: 1.4, ..Area::Lyrics.with(&state.prefs, i) };
            let mut tui = ui.new_child(UiBuilder::new().max_rect(card).id_salt(("first-run-lyrics-ui", i)));
            tui.set_clip_rect(ui.clip_rect().intersect(card));
            crate::screens::lyrics::preview_at(&tui, card, &shown, (!moving).then_some(1.25));
            let layer = if resp.is_pointer_button_down_on() {
                0.10
            } else if resp.hovered() {
                0.08
            } else {
                0.0
            };
            let layer = anim::fraction(&ctx, id.with("layer"), layer, theme.motion.fast_effects);
            if layer > 0.001 {
                ui.painter().rect_filled(card, radius, s.on_surface.gamma_multiply(layer));
            }
            let on = anim::fraction(&ctx, id.with("on"), if picked { 1.0 } else { 0.0 }, theme.motion.fast_effects);
            if on > 0.01 {
                ui.painter().rect_stroke(card, radius, egui::Stroke::new(2.0, s.primary.gamma_multiply(on)), egui::StrokeKind::Inside);
            }
            let color = mix(s.on_surface_variant, s.primary, on);
            ui.painter().text(pos2(card.center().x, card.bottom() + 8.0), Align2::CENTER_TOP, *name, TypeRole::LabelLarge.font_id(), color);
            if resp.clicked() && !picked {
                out.push(Command::SetPref(PrefPatch::Preset(Area::Lyrics, i)));
            }
        });
    }
}

/// Step 5: the theme, the name, the start with the system; and, when a
/// service with an export of the history was picked, the quiet way to
/// bring that history over.
fn more(ui: &mut Ui, slide: &Slide, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let p = &state.prefs;
    slide.part(ui, 1, |ui| {
        let themes = i18n::first_run::themes_in(i18n::locale());
        let w = themes.iter().map(|l| Button::new(*l).size(Size::S).natural_width(ui)).sum::<f32>() + 2.0 * 2.0;
        setting_row(ui, SettingId::Theme.label(), w, |ui| {
            let i = match p.theme {
                ThemeMode::Light => 0,
                ThemeMode::Dark => 1,
                ThemeMode::System => 2,
            };
            let (pick, _) = connected(ui, Id::new("first-run-theme"), &themes, i, &[]);
            if let Some(n) = pick {
                let mut after = p.clone();
                after.theme = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System][n.min(2)];
                out.push(Command::SetPref(PrefPatch::Edit { before: Box::new(p.clone()), after: Box::new(after) }));
            }
        });
    });
    slide.part(ui, 2, |ui| {
        let st = &mut pages.first_run;
        setting_row(ui, SettingId::UserName.label(), 240.0, |ui| {
            let field = TextField::new(&mut st.name, i18n::settings_tabs().name_hint).width(ui.available_width()).id_salt("first-run-name").show(ui);
            let name = nikgob_view::user_name(&st.name);
            if (field.submitted || field.response.lost_focus()) && !name.is_empty() && name != p.user_name {
                out.push(Command::SetPref(PrefPatch::UserName(name)));
                pages.name_synced = false;
            }
        });
    });
    slide.part(ui, 3, |ui| {
        setting_row(ui, SettingId::Autostart.label(), 52.0, |ui| {
            let mut on = p.autostart;
            if ui.add(Switch::new(&mut on).id_salt("first-run-autostart")).changed() {
                out.push(Command::SetPref(PrefPatch::Autostart(on)));
            }
        });
    });
    if pages.first_run.choices.picks.has_history() {
        ui.add_space(16.0);
        slide.part(ui, 4, |ui| history(ui, pages, out));
    }
}

/// A row of «Ещё немного»: the name at the left, the control (`width`
/// wide) at the right, both on the row's centre line; where both do not
/// fit (a narrow window), the control goes under the name.
fn setting_row(ui: &mut Ui, label: &str, width: f32, control: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    let w = ui.available_width();
    let g = crate::typography::layout(ui.painter(), TypeRole::BodyLarge.style().job(label, s.on_surface));
    let beside = g.size().x + 16.0 + width <= w;
    let (row, _) = ui.allocate_exact_size(vec2(w, if beside { 64.0 } else { g.size().y + 8.0 + 56.0 + 8.0 }), Sense::hover());
    let (text_y, control_rect) = if beside {
        (row.center().y - g.size().y / 2.0, Rect::from_min_max(pos2(row.right() - width, row.top()), row.max))
    } else {
        (row.top(), Rect::from_min_max(pos2(row.left(), row.top() + g.size().y + 8.0), pos2(row.left() + width.min(w), row.bottom())))
    };
    ui.painter().galley(pos2(row.left(), text_y), g, s.on_surface);
    let mut c = ui.new_child(UiBuilder::new().max_rect(control_rect).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt(("first-run-row", label)));
    control(&mut c);
}

/// «Перенести историю»: a text button; open, the buttons that order the
/// export where it is made and pick the archive (the import of Настройки →
/// «Сервисы»), with no words around them.
fn history(ui: &mut Ui, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let f = i18n::first_run();
    let st_tabs = i18n::settings_tabs();
    let st = &mut pages.first_run;
    let icon = if st.history { Icon::ExpandLess } else { Icon::ExpandMore };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), Size::S.height()), Sense::hover());
    let at = row.translate(vec2(-Size::S.side_padding(), 0.0));
    let mut c = ui.new_child(UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("first-run-history-row"));
    if c.add(Button::new(f.history_btn).icon(icon).style(Style::Text).size(Size::S).id_salt("first-run-history")).clicked() {
        st.history = !st.history;
    }
    let open = anim::fraction(ui.ctx(), Id::new("first-run-history-open"), if st.history { 1.0 } else { 0.0 }, Theme::get(ui.ctx()).motion.default_effects);
    if open < 0.01 {
        return;
    }
    let (spotify, youtube) = (st.choices.picks.has(Source::Spotify), st.choices.picks.has(Source::YouTube));
    ui.add_space(8.0);
    ui.scope(|ui| {
        ui.multiply_opacity(open);
        egui::Frame::new()
            .fill(s.surface_container_low)
            .corner_radius(egui::CornerRadius::same(shape::LARGE as u8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Where the export is ordered, then the archive once it
                // came: the buttons alone, one wrapped row.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    if spotify && ui.add(Button::new("Spotify").icon(Icon::OpenInNew).style(Style::Tonal).size(Size::S).id_salt("first-run-sp-export")).clicked() {
                        settings::open_url(SPOTIFY_EXPORT);
                    }
                    if youtube && ui.add(Button::new("Google Takeout").icon(Icon::OpenInNew).style(Style::Tonal).size(Size::S).id_salt("first-run-takeout")).clicked() {
                        settings::open_url(TAKEOUT);
                    }
                    if ui.add(Button::new(f.archive_btn).icon(Icon::UploadFile).style(Style::Tonal).size(Size::S).id_salt("first-run-import-zip")).clicked() {
                        super::dialogs::file(
                            |d| d.set_title(st_tabs.import_zip_title).add_filter(st_tabs.archive_filter, &["zip"]),
                            Command::ImportExportFiles,
                            out,
                        );
                    }
                    if ui.add(Button::new(st_tabs.folder_btn).icon(Icon::Folder).style(Style::Outlined).size(Size::S).id_salt("first-run-import-dir")).clicked() {
                        super::dialogs::folder(|d| d.set_title(st_tabs.import_dir_title), Command::ImportExportFiles, out);
                    }
                });
            });
    });
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picks(of: &[Source]) -> Choices {
        let mut c = Choices::default();
        for s in of {
            c.picks.toggle(*s);
        }
        c
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        ctx
    }

    /// One frame of the whole window `width` wide (a click at `click`),
    /// the prefs patches it sent applied; the text it painted.
    fn frame(ctx: &egui::Context, ui: &mut crate::screens::Md3eUi, state: &mut ViewState, time: f64, width: f32, click: Option<(egui::Pos2, bool)>) -> String {
        let events = click
            .map(|(pos, pressed)| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
                ]
            })
            .unwrap_or_default();
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 800.0))), time: Some(time), events, ..Default::default() },
            |root| ui.frame(root, state, &mut commands),
        );
        output.textures_delta.clear();
        for c in commands {
            if let Command::SetPref(patch) = c {
                patch.apply(&mut state.prefs);
            }
        }
        output.shapes.iter().map(|s| format!("{:?}", s.shape)).collect::<Vec<_>>().join("
")
    }

    /// Shown over the whole window while the prefs say so, at any width;
    /// ✕ ends it and the shell comes (here the empty Главная).
    #[test]
    fn it_covers_the_window_until_closed() {
        for width in [1280.0, 420.0] {
            let ctx = context();
            let mut ui = crate::screens::Md3eUi::new();
            let mut state = ViewState::default();
            state.prefs.first_run = true;
            state.home.library_empty = true;
            let mut page = String::new();
            for i in 0..30 {
                page = frame(&ctx, &mut ui, &mut state, f64::from(i) * 0.1, width, None);
            }
            assert!(page.contains("Откуда брать музыку?") && page.contains("Пропустить"), "{width}");
            assert!(!page.contains(super::super::home::EMPTY_LINE), "nothing of the shell under it at {width}");
            // ✕ at the top right.
            let close = pos2(width - if width < 600.0 { 16.0 } else { 24.0 } - 20.0, 36.0);
            frame(&ctx, &mut ui, &mut state, 3.1, width, Some((close, true)));
            frame(&ctx, &mut ui, &mut state, 3.2, width, Some((close, false)));
            assert!(!state.prefs.first_run, "closed at {width}");
            assert!(state.prefs.name_prompt == ViewState::default().prefs.name_prompt, "closed, not finished: the name is asked as before");
            let mut page = String::new();
            for i in 0..30 {
                page = frame(&ctx, &mut ui, &mut state, 4.0 + f64::from(i) * 0.1, width, None);
            }
            assert!(!page.contains("Откуда брать музыку?"), "{width}");
            if width >= 600.0 {
                assert!(page.contains(super::super::home::EMPTY_LINE), "the empty Главная at {width}");
            }
        }
    }

    #[test]
    fn the_steps_follow_the_picks() {
        assert_eq!(Step::path(&Choices::default()), [Step::Sources, Step::Keep, Step::Lyrics, Step::More], "skipped: nothing to connect");
        assert_eq!(Step::path(&picks(&[Source::YouTube])), [Step::Sources, Step::Keep, Step::Connect, Step::Lyrics, Step::More]);
        assert_eq!(Step::path(&picks(&[Source::Folder])), [Step::Sources, Step::Connect, Step::Lyrics, Step::More], "a folder keeps files");
        assert_eq!(Step::path(&picks(&[Source::LocalOnly])), [Step::Sources, Step::Lyrics, Step::More]);
    }
}
