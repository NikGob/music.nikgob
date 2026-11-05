//! The control under the lyrics and what opens behind it. The control
//! always opens its small menu ([`menu`]): the sources are picked there in
//! every look. Where the text is managed (seen by words or lines, timed,
//! saved) comes in three looks to choose from
//! (`NIKGOB_LYRICS_UI=menu|dialog|page`, the window by default): `menu` does
//! it all from the menu itself, [`dialog`] grows a modal window out of the
//! control, [`page`] opens the «Текст песни» page; their menu only picks
//! the source and opens them. Settings → «Текст песен» →
//! «Вид» picks the look (`Prefs::lyrics_ui`); `NIKGOB_LYRICS_UI` overrides
//! it for screenshot runs. Each look lives in its own
//! module; this one has what they share: the words, the control itself and
//! the progress of timing being made (docs/lyrics-align.md).
//!
//! The control is one tonal button: where the text comes from and how it
//! is timed («Genius · по словам»). While timing is made it shows its
//! progress and stage; a failure says why until it is opened.

use std::sync::OnceLock;

use egui::{Context, Id, Pos2, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use nikgob_motion::Spring;
use nikgob_view::{AlignStage, Command, LyricsAvail, LyricsCraft, LyricsHome, LyricsSource, LyricsSync, LyricsView, ViewState};

use crate::components::{LoadingIndicator, anim, paint, paint::Radii};
use crate::tokens::{shape, state};
use crate::{Icon, Theme, TypeRole};

mod dialog;
mod menu;
pub(crate) mod page;

pub(crate) use dialog::show as overlays;

/// Which look the control opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Menu,
    Dialog,
    Page,
}

impl Variant {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "menu" => Some(Self::Menu),
            "dialog" => Some(Self::Dialog),
            "page" => Some(Self::Page),
            _ => None,
        }
    }

    /// The look `Prefs::lyrics_ui` names (the menu for anything else).
    pub fn of(pref: u8) -> Self {
        match pref {
            1 => Self::Dialog,
            2 => Self::Page,
            _ => Self::Menu,
        }
    }
}

/// The look in use: `NIKGOB_LYRICS_UI` (read once) when it names one, else
/// the setting, which switches at once.
pub fn variant(prefs: &nikgob_view::Prefs) -> Variant {
    static ENV: OnceLock<Option<Variant>> = OnceLock::new();
    variant_or(*ENV.get_or_init(|| std::env::var("NIKGOB_LYRICS_UI").ok().as_deref().and_then(Variant::parse)), prefs)
}

fn variant_or(over: Option<Variant>, prefs: &nikgob_view::Prefs) -> Variant {
    over.unwrap_or_else(|| Variant::of(prefs.lyrics_ui))
}

/// The look the control was last drawn with (for [`open`]).
const SHOWN: &str = "lyrics-ui-variant";

/// What a look asks of the window (taken by `Md3eUi::frame`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// Settings at the «Текст песен» rows.
    Settings,
    /// The «Текст песни» page.
    Page,
}

const REQUEST: &str = "lyrics-ui-request";
/// The control drawn last: its id and rect (the dialog grows out of it).
const LAST: &str = "lyrics-ui-control";

pub(crate) fn request(ctx: &Context, r: Request) {
    ctx.data_mut(|d| d.insert_temp(Id::new(REQUEST), r));
}

pub(crate) fn take_request(ctx: &Context) -> Option<Request> {
    ctx.data_mut(|d| {
        let r = d.get_temp::<Request>(Id::new(REQUEST));
        d.remove::<Request>(Id::new(REQUEST));
        r
    })
}

/// The settings row «Настройки текста…» leads to.
pub(crate) const SETTINGS: super::pages::SettingId = super::pages::SettingId::LyricsUi;

/// Opens where the text is managed: the menu, the window or the page
/// (screenshot runs).
pub fn open(ctx: &Context) {
    let last = ctx.data(|d| d.get_temp::<(Id, Rect)>(Id::new(LAST)));
    match ctx.data(|d| d.get_temp::<Variant>(Id::new(SHOWN))).unwrap_or(Variant::Menu) {
        Variant::Menu => open_menu(ctx),
        Variant::Dialog => dialog::open(ctx, last.map_or(Rect::NOTHING, |(_, r)| r)),
        Variant::Page => request(ctx, Request::Page),
    }
}

/// Opens the control's own menu, in any look (screenshot runs).
pub fn open_menu(ctx: &Context) {
    if let Some((id, _)) = ctx.data(|d| d.get_temp::<(Id, Rect)>(Id::new(LAST))) {
        ctx.data_mut(|d| d.insert_temp(id.with("md3e-menu-open"), true));
    }
}

/// The control with its menu: centred in the width it is given, or with
/// its left edge at `left` (in line with the text). True when «Настройки
/// текста…» was picked.
pub fn control(ui: &mut Ui, state: &ViewState, left: Option<f32>, out: &mut Vec<Command>) -> bool {
    let v = variant(&state.prefs);
    ui.ctx().data_mut(|d| d.insert_temp(Id::new(SHOWN), v));
    menu::control(ui, state, v, left, out)
}

/// How a source's text is timed, for someone who never heard of timing
/// kinds: by its own times («синхронизирован по строкам») or by the
/// player's («плеер синхронизировал по словам»). Worded apart from how the
/// lyrics light up (by letters, by words), so the two are not confused.
pub(crate) fn timed(sync: LyricsSync, by_player: bool) -> String {
    let by = match sync {
        LyricsSync::Plain => return "без синхронизации".to_owned(),
        LyricsSync::Line => "по строкам",
        LyricsSync::Word => "по словам",
        LyricsSync::Syllable => "по слогам",
    };
    if by_player { format!("плеер синхронизировал {by}") } else { format!("синхронизирован {by}") }
}

/// A source's name where texts are listed.
pub(crate) fn name(source: LyricsSource) -> &'static str {
    match source {
        LyricsSource::File => "Файл трека",
        LyricsSource::Recognized => "Расслышал плеер",
        s => s.label(),
    }
}

/// What a song without a file calls the text it kept in its artist's
/// folder (it has no file to say it comes from).
const KEPT: &str = "Сохранённый текст";

/// [`name`] for the shown track.
pub(crate) fn source_name(l: &LyricsView, source: LyricsSource) -> &'static str {
    if source == LyricsSource::File && l.stream { KEPT } else { name(source) }
}

/// «Текст с Genius»: where the shown text came from, in plain words. How
/// it lights up is said only when it does not (a still text makes one
/// wonder); a text the player heard itself says so, so it is taken as a
/// guess.
pub(crate) fn chip_label(l: &LyricsView) -> String {
    let from = match l.source {
        Some(LyricsSource::Recognized) => "Текст расслышан плеером".to_owned(),
        Some(LyricsSource::File) if l.stream => KEPT.to_owned(),
        Some(LyricsSource::File) => "Текст из файла трека".to_owned(),
        Some(src) => format!("Текст с {}", src.label()),
        None if l.loading || l.sources.iter().any(|o| o.state == LyricsAvail::Loading) => return "Ищу текст…".to_owned(),
        None => return "Текста нет".to_owned(),
    };
    if l.lines.is_empty() { format!("{from} · без подсветки") } else { from }
}

/// How `source`'s text lights up as shown: the player's own timing when
/// it made one, else the source's.
fn kind(l: &LyricsView, source: LyricsSource) -> Option<LyricsSync> {
    let o = l.sources.iter().find(|o| o.source == source)?;
    let LyricsAvail::Ready(sync) = o.state else { return None };
    if o.fits == Some(false) {
        return None;
    }
    let made = l.texts.iter().find(|t| t.source == Some(source)).and_then(|t| t.made.as_deref()).filter(|_| o.made);
    Some(made.and_then(LyricsSync::of).unwrap_or(sync))
}

/// What a source's row says, where the sources are listed: whether it has
/// a text at all; how it is timed only where that differs between the
/// texts there are, and then also when the player timed it to the voice
/// itself.
pub(crate) fn source_state(l: &LyricsView, source: LyricsSource) -> String {
    let Some(o) = l.sources.iter().find(|o| o.source == source) else { return String::new() };
    match o.state {
        LyricsAvail::Loading => return "ищу…".to_owned(),
        LyricsAvail::Missing => return "нет текста".to_owned(),
        LyricsAvail::Ready(_) if o.fits == Some(false) => return "текст другой песни или версии".to_owned(),
        LyricsAvail::Ready(_) => {}
    }
    let kinds: Vec<LyricsSync> = l.sources.iter().filter_map(|o| kind(l, o.source)).collect();
    let differ = kinds.windows(2).any(|w| w[0] != w[1]);
    let Some(k) = kind(l, source).filter(|_| differ) else { return String::new() };
    timed(k, o.made && source != LyricsSource::Recognized)
}

/// `text` with a capital first letter (list rows).
pub(crate) fn capital(text: &str) -> String {
    let mut c = text.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// One timing that can be made for the shown text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Action {
    pub target: LyricsSync,
    /// Made with deep mode's parts for this song only; else the normal
    /// way, whatever the setting says.
    pub deep: bool,
    pub label: &'static str,
}

const fn action(target: LyricsSync, deep: bool, label: &'static str) -> Action {
    Action { target, deep, label }
}

/// The timing that can be made for the shown text, with what the action
/// says: lines for a plain text, words for a text without them, and the
/// words again for a text that has them (a bad result is made anew). The
/// words come twice: the normal way and deep (docs/lyrics-align.md, «Deep
/// mode»).
pub(crate) fn actions(state: &ViewState) -> Vec<Action> {
    let l = &state.lyrics;
    if l.craft != LyricsCraft::Ready || l.source.is_none_or(|s| s == LyricsSource::Recognized) {
        return Vec::new();
    }
    const WORDS: [Action; 2] =
        [action(LyricsSync::Word, false, "Подсветить по словам"), action(LyricsSync::Word, true, "Подсветить по словам глубоко")];
    // The finest timing the text has, however it is seen now.
    match l.shows.first().copied().or_else(|| LyricsSync::of(&l.lines)).unwrap_or(LyricsSync::Plain) {
        LyricsSync::Plain => [&[action(LyricsSync::Line, false, "Подсветить по строкам")][..], &WORDS[..]].concat(),
        LyricsSync::Line => WORDS.to_vec(),
        _ => vec![action(LyricsSync::Word, false, "Сделать подсветку заново"), action(LyricsSync::Word, true, "Сделать глубокую подсветку")],
    }
}

/// [`actions`] in the order the window and the page lay out their buttons
/// from the primary one: the normal word timing first (it leads), then
/// the others from the finest (deep beside its normal twin).
pub(crate) fn from_primary(actions: &[Action]) -> Vec<Action> {
    let Some(p) = actions.iter().rposition(|a| !a.deep) else { return actions.to_vec() };
    std::iter::once(actions[p]).chain(actions.iter().enumerate().rev().filter(|(i, _)| *i != p).map(|(_, a)| *a)).collect()
}

/// Why timing cannot be made for the shown text (a clip whose song is
/// unclear), where the action would be.
pub(crate) fn no_craft(state: &ViewState) -> Option<&str> {
    match &state.lyrics.craft {
        LyricsCraft::No(why) => Some(why),
        _ => None,
    }
}

/// Where the shown text is kept, in plain words, where that matters (the
/// window and the page): made by the player and kept in the library, or
/// saved on the computer. A text only online goes without saying.
pub(crate) fn home(l: &LyricsView) -> Option<&'static str> {
    l.source?;
    match l.home {
        LyricsHome::Online => None,
        LyricsHome::Library => Some("Сделано плеером, хранится в библиотеке"),
        LyricsHome::Computer => Some("Сохранено на компьютере"),
    }
}

/// How the shown text can be seen, with what the choice says: offered
/// only where there is a choice (a text with timing).
pub(crate) fn shows(state: &ViewState) -> Vec<(LyricsSync, &'static str)> {
    let l = &state.lyrics;
    if l.track.is_none() || l.shows.len() < 2 {
        return Vec::new();
    }
    l.shows.iter().map(|s| (*s, show_label(*s))).collect()
}

fn show_label(sync: LyricsSync) -> &'static str {
    match sync {
        LyricsSync::Word | LyricsSync::Syllable => "По словам",
        LyricsSync::Line => "По строкам",
        LyricsSync::Plain => "Без подсветки",
    }
}

/// Every way of seeing a text, finest first, with whether the shown one
/// has it (the window and the page show them all).
pub(crate) fn all_shows(state: &ViewState) -> [(LyricsSync, &'static str, bool); 3] {
    [LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain].map(|s| (s, show_label(s), state.lyrics.shows.contains(&s)))
}

/// How the shown text is seen now.
pub(crate) fn seen(l: &LyricsView) -> LyricsSync {
    match LyricsSync::of(&l.lines) {
        Some(LyricsSync::Syllable | LyricsSync::Word) => LyricsSync::Word,
        Some(LyricsSync::Line) => LyricsSync::Line,
        _ => LyricsSync::Plain,
    }
}

/// Sees the shown text as `sync` from now on.
pub(crate) fn show_as(state: &ViewState, sync: LyricsSync, out: &mut Vec<Command>) {
    if let Some(track) = state.lyrics.track.filter(|_| seen(&state.lyrics) != sync) {
        out.push(Command::ShowLyricsAs(track, sync));
    }
}

/// What keeping the shown text says: a song without a file is saved with
/// it when the settings say so («Что сохранять») and its song is clear
/// (a clip keeps only its text).
pub(crate) fn save_label(state: &ViewState) -> &'static str {
    let l = &state.lyrics;
    if l.stream && l.craft == LyricsCraft::Ready && state.prefs.lyrics_stream_keep == 1 { "Сохранить песню и текст" } else { "Сохранить текст" }
}

/// «Сохранить текст» is the primary action: for timing the player made
/// (a text from a source is kept as it is, so making timing stays first).
pub(crate) fn save_first(l: &LyricsView) -> bool {
    l.savable && l.home == LyricsHome::Library
}

/// Keeps the shown text on the computer.
pub(crate) fn save(state: &ViewState, out: &mut Vec<Command>) {
    if let (Some(track), Some(source), true) = (state.lyrics.track, state.lyrics.source, state.lyrics.savable) {
        out.push(Command::SaveLyrics { track, source });
    }
}

/// Asks for the timing `a` makes of the shown text.
pub(crate) fn make(state: &ViewState, a: Action, out: &mut Vec<Command>) {
    if let (Some(track), Some(source)) = (state.lyrics.track, state.lyrics.source) {
        out.push(Command::CraftLyrics { track, source, target: a.target, deep: a.deep });
    }
}

/// Timing being made for the shown track: its stage and how far along.
pub(crate) struct Busy {
    pub label: String,
    pub fraction: f32,
    pub stage: AlignStage,
}

pub(crate) fn busy(state: &ViewState) -> Option<Busy> {
    let track = state.lyrics.track?;
    state.align.craft.as_ref().filter(|c| c.track == track && c.failed.is_none() && c.lines.is_none())?;
    let job = state.align.job.as_ref().filter(|j| j.asked);
    let (stage, f, bytes) = job.map_or((AlignStage::Decode, 0.0, None), |j| (j.stage, j.progress, j.bytes));
    Some(Busy { label: stage_label(stage, bytes), fraction: overall(stage, f), stage })
}

/// Why the timing asked for could not be made.
pub(crate) fn failed(state: &ViewState) -> Option<&str> {
    let track = state.lyrics.track?;
    state.align.craft.as_ref().filter(|c| c.track == track).and_then(|c| c.failed.as_deref())
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

fn stage_label(stage: AlignStage, bytes: Option<(u64, u64)>) -> String {
    match (stage, bytes) {
        (AlignStage::Download, Some((done, total))) => format!("Скачиваю модели · {}", amount(done, total)),
        (AlignStage::Download, None) => "Скачиваю модели".to_owned(),
        (AlignStage::Decode, _) => "Читаю песню".to_owned(),
        (AlignStage::Separate, _) => "Отделяю голос".to_owned(),
        (AlignStage::Listen, _) => "Слушаю голос".to_owned(),
        (AlignStage::Align, _) => "Расставляю слова".to_owned(),
        (AlignStage::Recognise, _) => "Распознаю слова".to_owned(),
    }
}

/// «296 из 709 МБ», «0,4 из 1,8 ГБ»: both in the unit of the whole.
fn amount(done: u64, total: u64) -> String {
    let mb = |b: u64| b as f64 / 1_048_576.0;
    if mb(total) >= 1000.0 {
        let gb = |b: u64| format!("{:.1}", mb(b) / 1024.0).replace('.', ",");
        format!("{} из {} ГБ", gb(done), gb(total))
    } else {
        format!("{:.0} из {:.0} МБ", mb(done), mb(total))
    }
}

const HEIGHT: f32 = 32.0;
const ICON: f32 = 18.0;
const SLOT: f32 = ICON + 8.0;

/// What sits before the label.
#[derive(Clone, Copy, PartialEq)]
enum Lead {
    None,
    Progress(f32),
    Error,
}

/// The control: a tonal extra-small button in a row as wide as `ui`, on its
/// centre line or from `at` (following it on a spring, kept inside the
/// row). `open` (0..1) turns its chevron over while what it opens is open.
/// A click on a failure clears it (it was seen).
pub(crate) fn chip(ui: &mut Ui, state: &ViewState, open: f32, at: Option<f32>, out: &mut Vec<Command>) -> Response {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = ui.id().with("lyrics-control");
    let (label, lead) = match (busy(state), failed(state)) {
        (Some(b), _) => (b.label, Lead::Progress(b.fraction)),
        (None, Some(e)) => (e.to_owned(), Lead::Error),
        _ => (chip_label(&state.lyrics), Lead::None),
    };
    let content = if lead == Lead::Error { s.error } else { s.on_secondary_container };
    let galley = crate::typography::layout(ui.painter(), TypeRole::LabelLarge.style().job(label.clone(), content));

    // The leading slot opens and closes, the width follows the label.
    let slot = anim::f32(&ctx, id.with("slot"), if lead == Lead::None { 0.0 } else { SLOT }, motion.fast_spatial).max(0.0);
    let slot_k = (slot / SLOT).clamp(0.0, 1.0);
    let left = 12.0 - 4.0 * slot_k;
    let target_w = left + slot + galley.size().x + 4.0 + ICON + 10.0;
    let w = anim::f32(&ctx, id.with("w"), target_w, motion.fast_spatial).max(HEIGHT);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
    let visual = match at {
        Some(x) => {
            let x = anim::f32(&ctx, id.with("x"), x, motion.default_spatial);
            let x = x.min(row.right() - w).max(row.left());
            Rect::from_min_size(pos2(x, row.center().y - HEIGHT * 0.5), vec2(w, HEIGHT))
        }
        None => Rect::from_center_size(row.center(), vec2(w, HEIGHT)),
    };
    let response = ui
        .interact(visual.expand2(vec2(0.0, 8.0)), id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ctx.data_mut(|d| d.insert_temp(Id::new(LAST), (id, visual)));
    if response.clicked() && lead == Lead::Error {
        out.push(Command::ClearCraft);
    }

    // A new label fades in.
    let shown_key = id.with("label");
    let before: Option<String> = ctx.data(|d| d.get_temp(shown_key));
    if before.as_deref() != Some(label.as_str()) {
        ctx.data_mut(|d| d.insert_temp(shown_key, label.clone()));
        if before.is_some() {
            anim::fraction(&ctx, id.with("fade"), 0.0, motion.default_effects);
            anim::snap_f32(&ctx, id.with("fade"), 0.0);
        }
    }
    let fade = anim::fraction(&ctx, id.with("fade"), 1.0, motion.default_effects).clamp(0.0, 1.0);

    if ui.is_rect_visible(visual) {
        // Round at rest, squarer while pressed (Expressive buttons).
        let pressed = response.is_pointer_button_down_on();
        let radius = anim::f32(&ctx, id.with("radius"), if pressed { shape::SMALL } else { HEIGHT * 0.5 }, motion.fast_spatial);
        let radii = Radii::all(radius).clamped(visual);
        let painter = ui.painter();
        paint::container(painter, visual, radii, s.secondary_container, Stroke::NONE);
        let layer_target = if pressed {
            state::PRESSED
        } else if response.hovered() {
            state::HOVER
        } else {
            0.0
        };
        let layer = anim::fraction(&ctx, id.with("layer"), layer_target, motion.fast_effects);
        paint::state_layer(painter, visual, radii, s.on_secondary_container, layer);

        let mut x = visual.left() + left;
        let cy = visual.center().y;
        let lead_rect = Rect::from_center_size(pos2(x + ICON * 0.5, cy), vec2(ICON, ICON));
        match lead {
            Lead::Progress(f) => {
                let f = anim::f32(&ctx, id.with("progress"), f, Spring::new(1.0, 120.0)).clamp(0.0, 1.0);
                ring(painter, lead_rect.center(), ICON * 0.5 - 1.5, f, s.on_secondary_container, slot_k);
                ctx.request_repaint();
            }
            Lead::Error => {
                Icon::Error.widget(false, s.error.gamma_multiply(slot_k), ICON).paint_at(ui, lead_rect);
            }
            Lead::None => {}
        }
        x += slot;
        let painter = ui.painter();
        painter.galley(pos2(x, cy - galley.size().y * 0.5), galley.clone(), content.gamma_multiply(fade));
        let chevron = Rect::from_center_size(pos2(visual.right() - 10.0 - ICON * 0.5, cy), vec2(ICON, ICON));
        Icon::ExpandMore
            .widget(false, s.on_secondary_container, ICON)
            .rotate(std::f32::consts::PI * open.clamp(0.0, 1.0), vec2(0.5, 0.5))
            .paint_at(ui, chevron);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    response
}

/// A small determinate circular indicator: the active arc in `color`, the
/// rest of the track faint, a gap between them (M3 circular progress).
fn ring(painter: &egui::Painter, c: Pos2, r: f32, f: f32, color: egui::Color32, alpha: f32) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let stroke = 2.5;
    let gap = 0.5;
    let arc = |from: f32, to: f32, col: egui::Color32| {
        if to - from < 0.01 {
            return;
        }
        let n = ((to - from) / TAU * 48.0).ceil().max(2.0) as usize;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32 - FRAC_PI_2;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(stroke, col)));
    };
    let sweep = (f.max(0.03)) * TAU;
    arc(0.0, sweep, color.gamma_multiply(alpha));
    if sweep + 2.0 * gap < TAU {
        arc(sweep + gap, TAU - gap, color.gamma_multiply(0.24 * alpha));
    }
}

/// The work under way, for the dialog and the page: the Expressive loading
/// shape in its container, the stage (cross-fading when it changes) and a
/// wavy bar on a spring.
pub(crate) fn progress(ui: &mut Ui, b: &Busy, width: f32) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let shown = anim::f32(&ctx, ui.id().with("lyrics-ui-progress"), b.fraction, Spring::new(1.0, 120.0)).clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.add(LoadingIndicator::new().contained(true).size(48.0));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(4.0);
            let fade = anim::fraction(&ctx, ui.id().with(("stage", b.stage as u8)), 1.0, theme.motion.default_effects).clamp(0.0, 1.0);
            ui.scope(|ui| {
                ui.multiply_opacity(fade);
                ui.label(TypeRole::TitleSmall.style().rich(&b.label, s.on_surface));
            });
            ui.add_space(10.0);
            crate::components::wavy_progress(ui, "lyrics-ui-wave", shown, (width - 64.0).max(80.0));
        });
    });
    ctx.request_repaint();
}

/// The shown text in full in a card of `size`: following the song when it
/// is timed (a click seeks), else as plain text that scrolls.
pub(crate) fn text_card(ui: &mut Ui, size: egui::Vec2, state: &ViewState, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, shape::LARGE, s.surface_container);
    let l = &state.lyrics;
    // Another text cross-fades in and rises a little.
    let key = (l.source.map(LyricsSource::key), LyricsSync::of(&l.lines));
    let id = ui.id().with("lyrics-ui-text");
    let before: Option<(Option<&'static str>, Option<LyricsSync>)> = ctx.data(|d| d.get_temp(id.with("key")));
    if before != Some(key) {
        ctx.data_mut(|d| d.insert_temp(id.with("key"), key));
        if before.is_some() {
            anim::fraction(&ctx, id.with("k"), 0.0, theme.motion.default_spatial);
            anim::snap_f32(&ctx, id.with("k"), 0.0);
        }
    }
    let k = anim::fraction(&ctx, id.with("k"), 1.0, theme.motion.default_spatial);
    let inner = rect.shrink2(vec2(24.0, 16.0)).translate(vec2(0.0, (1.0 - k.clamp(0.0, 1.2)) * 16.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("lyrics-ui-text-body"));
    child.set_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
    child.multiply_opacity(k.clamp(0.0, 1.0));
    if !l.lines.is_empty() {
        super::lyrics::made_preview(&mut child, inner.size(), state, &l.lines, out);
    } else if let Some(text) = l.plain.as_deref() {
        egui::ScrollArea::vertical().id_salt("lyrics-ui-plain").auto_shrink([false, false]).show(&mut child, |ui| {
            ui.label(TypeRole::BodyLarge.style().rich_para(text.trim(), s.on_surface));
        });
    } else {
        let text = if l.loading { "Ищу текст…" } else { "Текста нет" };
        child.label(TypeRole::BodyLarge.style().rich(text, s.on_surface_variant));
    }
}

/// «15 Step · Radiohead»: what is playing.
pub(crate) fn playing(state: &ViewState) -> Option<String> {
    state.player.current.as_ref().map(|t| format!("{} · {}", t.title, t.artist))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::{LyricsOption, LyricsText};

    #[test]
    fn the_look_follows_the_setting_and_names_parse() {
        assert_eq!(Variant::of(0), Variant::Menu);
        assert_eq!(Variant::of(1), Variant::Dialog);
        assert_eq!(Variant::of(2), Variant::Page);
        assert_eq!(Variant::of(9), Variant::Menu);
        assert_eq!(Variant::parse("page"), Some(Variant::Page));
        assert_eq!(Variant::parse(""), None);
        // The default is the window; the setting picks the others.
        let mut p = nikgob_view::Prefs::default();
        assert_eq!(variant_or(None, &p), Variant::Dialog);
        p.lyrics_ui = 2;
        assert_eq!(variant_or(None, &p), Variant::Page);
        // The screenshot override wins.
        assert_eq!(variant_or(Some(Variant::Dialog), &p), Variant::Dialog);
    }

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
    fn downloads_say_how_much_in_the_unit_of_the_whole() {
        assert_eq!(stage_label(AlignStage::Download, Some((310_378_496, 743_440_384))), "Скачиваю модели · 296 из 709 МБ");
        assert_eq!(amount(429_496_730, 1_932_735_283), "0,4 из 1,8 ГБ");
        assert_eq!(stage_label(AlignStage::Listen, None), "Слушаю голос");
    }

    #[test]
    fn labels_say_the_source_and_how_it_goes_and_who_timed_it_only_in_the_list() {
        use std::time::Duration;
        let mut line = nikgob_view::LyricLine::new(Duration::from_secs(1), "Тёплое место");
        let lines = vec![line.clone()];
        let o = |source, sync, fits, made| LyricsOption { source, state: LyricsAvail::Ready(sync), fits, made };
        let mut l = LyricsView {
            source: Some(LyricsSource::Genius),
            lines: lines.clone(),
            made: true,
            sources: vec![
                o(LyricsSource::Genius, LyricsSync::Plain, Some(true), true),
                o(LyricsSource::Lrclib, LyricsSync::Line, Some(false), false),
                LyricsOption { source: LyricsSource::LyricsPlus, state: LyricsAvail::Missing, fits: None, made: false },
            ],
            texts: vec![LyricsText { source: Some(LyricsSource::Genius), made: Some(lines.clone()), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(chip_label(&l), "Текст с Genius");
        // The only text there is: how it lights up goes without saying.
        assert_eq!(source_state(&l, LyricsSource::Genius), "");
        assert_eq!(source_state(&l, LyricsSource::Lrclib), "текст другой песни или версии");
        assert_eq!(source_state(&l, LyricsSource::LyricsPlus), "нет текста");
        // Another text that lights up otherwise: now each row says how.
        l.sources.push(o(LyricsSource::LrcRed, LyricsSync::Syllable, Some(true), false));
        assert_eq!(source_state(&l, LyricsSource::Genius), "плеер синхронизировал по строкам");
        assert_eq!(source_state(&l, LyricsSource::LrcRed), "синхронизирован по слогам");
        assert_eq!(capital("по строкам"), "По строкам");
        line.words.push(nikgob_view::LyricWord { start: Duration::from_secs(1), end: Duration::from_secs(2), text: "Тёплое ".into() });
        l.lines = vec![line];
        assert_eq!(chip_label(&l), "Текст с Genius");
        l.lines.clear();
        assert_eq!(chip_label(&l), "Текст с Genius · без подсветки");
        l.source = Some(LyricsSource::Recognized);
        l.lines = lines.clone();
        assert_eq!(chip_label(&l), "Текст расслышан плеером");
        l.source = None;
        l.lines.clear();
        assert_eq!(chip_label(&l), "Текста нет");
        // No internal words anywhere, and nothing that reads like how the
        // lyrics light up.
        for w in [timed(LyricsSync::Line, false), timed(LyricsSync::Word, true), name(LyricsSource::Recognized).to_owned()] {
            assert!(!w.contains("распознан") && !w.contains("сделал") && !w.contains("подсвет") && !w.contains("целиком"), "{w}");
        }
        assert_eq!(timed(LyricsSync::Plain, true), "без синхронизации");
    }

    #[test]
    fn actions_make_what_the_shown_text_lacks() {
        use std::time::Duration;
        let made = |state: &ViewState| actions(state).iter().map(|a| (a.target, a.deep, a.label)).collect::<Vec<_>>();
        let mut state = ViewState::default();
        state.lyrics.source = Some(LyricsSource::Genius);
        state.lyrics.plain = Some("a".into());
        assert!(actions(&state).is_empty(), "nothing to listen to");
        state.lyrics.craft = LyricsCraft::Ready;
        // A plain text: lines, words, and words deep (lines have no twin).
        assert_eq!(
            made(&state),
            [
                (LyricsSync::Line, false, "Подсветить по строкам"),
                (LyricsSync::Word, false, "Подсветить по словам"),
                (LyricsSync::Word, true, "Подсветить по словам глубоко"),
            ]
        );
        let mut line = nikgob_view::LyricLine::new(Duration::from_secs(1), "a");
        state.lyrics.lines = vec![line.clone()];
        assert_eq!(made(&state), [(LyricsSync::Word, false, "Подсветить по словам"), (LyricsSync::Word, true, "Подсветить по словам глубоко")]);
        // Word timing there already: it can be made again, either way.
        let again = [(LyricsSync::Word, false, "Сделать подсветку заново"), (LyricsSync::Word, true, "Сделать глубокую подсветку")];
        line.words.push(nikgob_view::LyricWord { start: Duration::from_secs(1), end: Duration::from_secs(2), text: "a".into() });
        state.lyrics.lines = vec![line];
        assert_eq!(made(&state), again);
        // Seen by lines, the words are still there: made anew, not again.
        state.lyrics.shows = vec![LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain];
        state.lyrics.lines.iter_mut().for_each(|l| l.words.clear());
        assert_eq!(made(&state), again);
        state.lyrics.source = Some(LyricsSource::Recognized);
        assert!(actions(&state).is_empty());
        // A clip whose song is unclear: why, instead of the action.
        state.lyrics.source = Some(LyricsSource::Genius);
        state.lyrics.craft = LyricsCraft::No("Нельзя: это клип".into());
        assert!(actions(&state).is_empty());
        assert_eq!(no_craft(&state), Some("Нельзя: это клип"));
    }

    /// Each row asks for its own mode; the window and the page lead with
    /// the normal word timing, deep beside it.
    #[test]
    fn the_deep_rows_ask_for_deep_timing_and_the_normal_ones_lead() {
        let mut state = ViewState::default();
        state.lyrics.track = Some(nikgob_view::TrackId::new());
        state.lyrics.source = Some(LyricsSource::Genius);
        state.lyrics.craft = LyricsCraft::Ready;
        let all = actions(&state);
        let mut out = Vec::new();
        for a in &all {
            make(&state, *a, &mut out);
        }
        let asked: Vec<_> = out
            .iter()
            .map(|c| match c {
                Command::CraftLyrics { source: LyricsSource::Genius, target, deep, .. } => (*target, *deep),
                c => panic!("{c:?}"),
            })
            .collect();
        assert_eq!(asked, [(LyricsSync::Line, false), (LyricsSync::Word, false), (LyricsSync::Word, true)]);
        let order: Vec<_> = from_primary(&all).iter().map(|a| a.label).collect();
        assert_eq!(order, ["Подсветить по словам", "Подсветить по словам глубоко", "Подсветить по строкам"]);
        let again = [action(LyricsSync::Word, false, "Сделать подсветку заново"), action(LyricsSync::Word, true, "Сделать глубокую подсветку")];
        assert_eq!(from_primary(&again), again);
        assert!(from_primary(&[]).is_empty());
    }

    #[test]
    fn a_text_with_timing_offers_how_to_see_it() {
        use std::time::Duration;
        let mut state = ViewState::default();
        state.lyrics.track = Some(nikgob_view::TrackId::new());
        state.lyrics.shows = vec![LyricsSync::Plain];
        assert!(shows(&state).is_empty(), "a plain text: no choice");
        state.lyrics.shows = vec![LyricsSync::Word, LyricsSync::Line, LyricsSync::Plain];
        assert_eq!(shows(&state).iter().map(|s| s.1).collect::<Vec<_>>(), ["По словам", "По строкам", "Без подсветки"]);
        state.lyrics.shows = vec![LyricsSync::Line, LyricsSync::Plain];
        assert_eq!(all_shows(&state).map(|s| s.2), [false, true, true]);
        // Seen as it is now; picking that again asks for nothing.
        assert_eq!(seen(&state.lyrics), LyricsSync::Plain);
        state.lyrics.lines = vec![nikgob_view::LyricLine::new(Duration::from_secs(1), "a")];
        assert_eq!(seen(&state.lyrics), LyricsSync::Line);
        let mut out = Vec::new();
        show_as(&state, LyricsSync::Line, &mut out);
        assert!(out.is_empty());
        show_as(&state, LyricsSync::Plain, &mut out);
        assert!(matches!(out[..], [Command::ShowLyricsAs(_, LyricsSync::Plain)]));
    }

    #[test]
    fn the_window_says_where_the_text_is_kept_where_it_matters() {
        let mut l = LyricsView::default();
        assert_eq!(home(&l), None, "no text, nothing to say");
        l.source = Some(LyricsSource::Lrclib);
        assert_eq!(home(&l), None, "only online goes without saying");
        l.home = LyricsHome::Library;
        assert_eq!(home(&l), Some("Сделано плеером, хранится в библиотеке"));
        l.home = LyricsHome::Computer;
        assert_eq!(home(&l), Some("Сохранено на компьютере"));
        let mut state = ViewState::default();
        state.lyrics = l;
        state.lyrics.track = Some(nikgob_view::TrackId::new());
        let mut out = Vec::new();
        save(&state, &mut out);
        assert!(out.is_empty(), "not savable");
        state.lyrics.savable = true;
        save(&state, &mut out);
        assert!(matches!(out[..], [Command::SaveLyrics { source: LyricsSource::Lrclib, .. }]));
    }

    /// Any text not on the computer can be saved; only timing the player
    /// made puts «Сохранить текст» first.
    #[test]
    fn saving_comes_first_only_for_timing_the_player_made() {
        let mut l = LyricsView { source: Some(LyricsSource::Lrclib), savable: true, ..Default::default() };
        assert!(!save_first(&l), "a source's text: making timing stays first");
        l.home = LyricsHome::Library;
        assert!(save_first(&l));
        l.savable = false;
        assert!(!save_first(&l));
    }


    /// A song without a file says when keeping its text saves the song too.
    #[test]
    fn keeping_a_stream_s_text_says_whether_the_song_goes_too() {
        let mut state = ViewState::default();
        assert_eq!(save_label(&state), "Сохранить текст");
        state.prefs.lyrics_stream_keep = 1;
        assert_eq!(save_label(&state), "Сохранить текст", "a file's text");
        state.lyrics.stream = true;
        assert_eq!(save_label(&state), "Сохранить текст", "a clip: only its text");
        state.lyrics.craft = LyricsCraft::Ready;
        assert_eq!(save_label(&state), "Сохранить песню и текст");
        state.prefs.lyrics_stream_keep = 0;
        assert_eq!(save_label(&state), "Сохранить текст");
        // Its kept text is no file's.
        state.lyrics.source = Some(LyricsSource::File);
        assert_eq!(source_name(&state.lyrics, LyricsSource::File), "Сохранённый текст");
        assert_eq!(source_name(&state.lyrics, LyricsSource::Genius), "Genius");
        state.lyrics.stream = false;
        assert_eq!(source_name(&state.lyrics, LyricsSource::File), "Файл трека");
    }
}
