//! Now Playing: morphing cover inside a dotted ring
//! that turns and follows the spectrum, wavy progress, the hero play button
//! in a standard button group, one content slot, and the ambient
//! background (soft colour clouds following the bass).
//!
//! Wide windows put the cover on the left and the text and controls beside
//! it; narrow ones stack them. The content is laid out for the full window
//! even while the container grows out of the floating player, so nothing
//! reflows during the transition (the shell scales and fades it instead).

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_motion::Animated;
use nikgob_view::{Command, CoverShape, LikeTarget, NpSlot, Repeat, RingStyle, Status, ViewState};

use super::{
    chrome::{self, Act, Chrome, WheelZone, Where},
    common,
    pages::{PageState, sound},
    pane,
    stage::{Part, Stage},
};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        ButtonGroup, IconButton, Item, Size, Slider, Style, anim,
        button::{Colors, Shape as BtnShape},
        paint::{self, Radii},
    },
    size_class,
};

mod ring;
mod shapes;

pub use shapes::{Outline, cover_shape};
pub(crate) use ring::preview;
pub(crate) use shapes::{material, pair_morph, random_pool, reroll_shape, shape_cubics};

use self::ring::{RING_PAD, RingInput, cava, pulse, ring, ring_on, ring_shown, turn};
use self::shapes::{change_shape, cover_angle, cover_shape_in, shape_swap};

#[cfg(test)]
use self::ring::{BAR_COUNT, Cava, cava_targets, circle_path, dot_phase};
#[cfg(test)]
use self::shapes::{BINS, CoverTurn, cover_morph, square_pose, symmetry};
#[cfg(test)]
use nikgob_shapes::{MaterialShape, Point};
#[cfg(test)]
use std::f32::consts::TAU;

/// Which single tap is waiting out the double click delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tap {
    Pause,
}

/// The container under Now Playing: the floating pill's colour and radius
/// turning into the full surface as `k` goes 0 → 1.
pub fn container(ui: &Ui, rect: Rect, k: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    let kc = k.clamp(0.0, 1.0);
    // The pill's own radius going to square corners; a full radius on the
    // growing rect would leave the content's rectangular clip poking out
    // of big round ends.
    let radius = (crate::components::toolbar::HEIGHT * 0.5).min(rect.height() * 0.5) * (1.0 - kc);
    let bg = crate::lerp_color(s.primary_container, s.surface, kc);
    paint::container(ui.painter(), rect, Radii::all(radius), bg, Stroke::NONE);
}

/// `r` with its corner on the pixel grid (crisp text, no "Unaligned").
fn snap(ui: &Ui, r: Rect) -> Rect {
    let ppp = ui.ctx().pixels_per_point();
    let min = pos2((r.min.x * ppp).round() / ppp, (r.min.y * ppp).round() / ppp);
    Rect::from_min_size(min, r.size())
}

/// Draws Now Playing over `rect` (the whole window). `embedded` = mini mode,
/// with no close button. Returns true when the user closes it (the close
/// button, a click on empty space, a swipe down, or a navigation chosen
/// from a menu).
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    state: &ViewState,
    embedded: bool,
    pages: &mut PageState,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) -> bool {
    show_on(ui, rect, state, embedded, pages, chrome, &Stage::REAL, out)
}

/// [`show`] for the real screen ([`Stage::REAL`]) or the settings' stage:
/// the copy keeps springs of its own, reads no pointer, claims neither the
/// wheel nor the snackbars' spot, plays on the levels it is given and
/// notes where its parts are. (Its commands are the caller's to drop.)
#[allow(clippy::too_many_arguments)]
pub fn show_on(
    ui: &mut Ui,
    rect: Rect,
    state: &ViewState,
    embedded: bool,
    pages: &mut PageState,
    chrome: &mut Chrome,
    stage: &Stage,
    out: &mut Vec<Command>,
) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let p = &state.player;
    let playing = p.status == Status::Playing;
    let mut closed = false;
    let levels_bass = stage.bass(pages.levels_bass);
    let bands = stage.bands(&chrome.bands);

    // The layer must know its size, or the wheel scrolls the page under it.
    ui.expand_to_include_rect(rect);
    // Swallow input under the overlay; a click on empty space or a swipe
    // down closes it.
    let bg = ui.interact(rect, ui.id().with("np-bg"), Sense::click_and_drag());
    if !embedded
        && stage.live
        && bg.drag_stopped()
        && let Some(d) = bg.total_drag_delta()
        && d.y > 96.0
        && d.y > d.x.abs() * 1.5
    {
        closed = true;
    }
    {
        let _p = crate::prof::scope("np.ambient");
        ambient(ui, rect, state, levels_bass, stage);
    }
    let motion = !theme.reduced_motion;
    let bass = if playing { levels_bass } else { 0.0 };
    // The ring comes in a little slower than it goes, both on the
    // critically damped effects springs (no overshoot), and its size never
    // changes the cover's.
    let ring_shown = ring_shown(state.prefs.ring, state.prefs.cover_shape, playing);
    let ring_spring = if ring_shown { theme.motion.slow_effects } else { theme.motion.default_effects };
    let ring_k = anim::fraction(&ctx, stage.id("np-ring-shown"), if ring_shown { 1.0 } else { 0.0 }, ring_spring).clamp(0.0, 1.0);
    // While the ring fades on a pause its dots fall back at their own pace.
    let (kick, transients) = if motion && (playing || ring_k > 0.01) {
        pulse(ui, stage.id("np-pulse"), bass, &bands, playing)
    } else {
        (0.0, Vec::new())
    };
    let (live_kick, live_transients): (f32, &[f32]) = if playing { (kick, &transients) } else { (0.0, &[]) };
    {
        let _p = crate::prof::scope("np.bg_shapes");
        super::bg_shapes::paint(ui, stage.id("np-bg-shapes"), rect, &state.prefs, playing && stage.moving, live_kick, live_transients, bass);
    }
    stage.part(&ctx, Part::Background, rect);

    let class = size_class(rect.width());
    let margin = class.margin();
    let inner = rect.shrink2(vec2(margin, 16.0));
    // Content scrolls; the wheel elsewhere changes volume.
    if stage.live {
        chrome.volume_zone(rect, WheelZone::Np);
    }

    // Header.
    let header = snap(ui, Rect::from_min_size(inner.min, vec2(inner.width(), 48.0)));
    let mut hui = ui.new_child(
        UiBuilder::new()
            .max_rect(header)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if !embedded
        && hui
            .add(IconButton::new(Icon::ExpandMore).tooltip(nikgob_core::i18n::player().collapse_tooltip).id_salt("np-close"))
            .clicked()
    {
        closed = true;
    }
    let (ctx_text, ctx_link) = p.cached_context();
    let left_used = hui.min_rect().right() - header.left();
    let right_used = hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = ui.add(IconButton::new(Icon::MoreVert).tooltip(nikgob_core::i18n::common().more).id_salt("np-more"));
        if let Some(t) = p.current.as_ref() {
            // The track's one list, the visualizer before it, the delete
            // last.
            // «Текст песни» while its button is hidden (L opens it too).
            let lyrics = (!state.prefs.player_buttons.lyrics).then_some(Act::Lyrics);
            // «Где играть» while the content is open (its layouts have no
            // row for the button) and this computer plays: another
            // device's pill stands under the progress bar then.
            let output = (chrome.np_slot != NpSlot::None && state.target.is_none() && super::output::available(state)).then_some(Act::Output);
            let top: Vec<Act> = lyrics.into_iter().chain(output).chain([Act::Visualizer, Act::SleepMenu]).collect();
            let rows = Chrome::player_rows(&ctx, t, state, &top, true, true);
            chrome.track_menu(&more, "np-more", rows, t, state, Where::Np, out);
        }
        super::sleep_ui::chip(ui, state, "np-sleep", s.secondary_container, s.on_secondary_container, class == crate::tokens::SizeClass::Compact, out);
        // Q and L work with the buttons hidden (Settings → Вид).
        let buttons = state.prefs.player_buttons;
        if buttons.queue && ui.add(
            IconButton::new(Icon::QueueMusic)
                .style(Style::Tonal)
                .selected(chrome.np_slot == NpSlot::Queue)
                .tooltip(nikgob_core::i18n::player().queue_tooltip)
                .id_salt("np-queue-btn"),
        ).clicked() {
            chrome.np_slot.toggle(NpSlot::Queue);
        }
        if buttons.lyrics && ui.add(
            IconButton::new(Icon::Lyrics)
                .style(Style::Tonal)
                .selected(chrome.np_slot == NpSlot::Lyrics)
                .tooltip(nikgob_core::i18n::player().lyrics_tooltip)
                .id_salt("np-lyrics-btn"),
        ).clicked() {
            chrome.np_slot.toggle(NpSlot::Lyrics);
        }
        header.right() - ui.min_rect().left()
    }).inner;
    // Centred on the whole screen, not in the gap between the buttons.
    let side = left_used.max(right_used).max(0.0) + 8.0;
    let title = Rect::from_min_max(pos2(header.left() + side, header.top()), pos2(header.right() - side, header.bottom()));
    if title.width() > 40.0 {
        let mut tui = ui.new_child(
            UiBuilder::new()
                .max_rect(title)
                .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        // The album, playlist or artist it plays from: its name is a link to
        // its page, like the album under the title.
        match (ctx_link, p.context_page.clone()) {
            (Some(name), Some(page)) => {
                let format = TypeRole::TitleSmall.style().text_format(s.on_surface_variant);
                if crate::components::link_label(&mut tui, "np-context", &ctx_text, &[name], format).clicked.is_some() {
                    chrome.nav = Some(page);
                }
            }
            _ => {
                common::label(&mut tui, &ctx_text, TypeRole::TitleSmall, s.on_surface_variant);
            }
        }
    }

    let body = Rect::from_min_max(pos2(inner.left(), header.bottom() + 16.0), inner.max);
    let slot_open = chrome.np_slot != NpSlot::None;
    let mega_k = anim::fraction(&ctx, stage.id("np-mega"), if slot_open { 1.0 } else { 0.0 }, theme.motion.default_spatial).clamp(0.0, 1.0);
    let content_alpha = slot_opacities(&ctx, chrome.np_slot, stage);
    let main = body;

    // Layout: cover beside the text when there is room, above it otherwise.
    let info_h = INFO_H;
    let wide = main.width() >= 720.0 && main.height() >= 320.0;
    let (cover_box, info_rect) = if wide {
        let info_w = (main.width() * 0.42).clamp(360.0, 520.0);
        let gap = 40.0;
        let cover = (main.width() - info_w - gap)
            .min(main.height() - 16.0)
            .clamp(160.0, 600.0);
        let total = cover + gap + info_w;
        let left = main.center().x - total * 0.5;
        let cy = main.center().y;
        (
            Rect::from_center_size(pos2(left + cover * 0.5, cy), vec2(cover, cover)),
            // A touch below centre: the big title makes the block top-heavy.
            Rect::from_min_size(pos2(left + cover + gap, cy - info_h * 0.5 + 20.0), vec2(info_w, info_h)),
        )
    } else {
        let col_w = main.width().min(560.0);
        let cover = (main.height() - info_h - 24.0)
            .min(col_w)
            .clamp(120.0, 560.0);
        let block = cover + 24.0 + info_h;
        let top = main.top() + ((main.height() - block) * 0.5).max(0.0);
        let cx = main.center().x;
        (
            Rect::from_center_size(pos2(cx, top + cover * 0.5), vec2(cover, cover)),
            Rect::from_min_size(pos2(cx - col_w * 0.5, top + cover + 24.0), vec2(col_w, info_h)),
        )
    };

    if stage.live {
        snack_spot(&ctx, rect, wide, slot_open, cover_box, info_rect);
    }

    // Cover: playing → its shape, paused → rounded square (with «Менять
    // форму на паузе» off it keeps the shape, only still); the ring follows
    // the outline through the morph.
    let prefs = &state.prefs;
    let morph = anim::fraction(
        &ctx,
        stage.id("np-cover-morph"),
        if playing || !prefs.cover_pause_morph { 1.0 } else { 0.0 },
        theme.motion.default_spatial,
    );
    let mega = mega_layout(body);
    // Whether it plays or not: a pause never resizes the cover.
    let bars = ring_on(prefs.ring, prefs.cover_shape) && prefs.ring_style == RingStyle::Bars;
    let current = p.current.as_ref();
    let shape = cover_shape_in(&ctx, stage.id("np-random-shape"), prefs.cover_shape, &prefs.random_shapes, current);
    // The bars want room to grow: the cover gives way to them.
    let pad = if bars { (cover_box.width() * 0.13).clamp(RING_PAD, 80.0) } else { RING_PAD };
    let normal_cover = cover_box.shrink(pad);
    let cover_rect = if theme.reduced_motion { normal_cover } else { lerp_rect(normal_cover, mega.cover, mega_k) };
    let swap = shape_swap(&ctx, stage, shape, morph >= 0.999, theme.reduced_motion);
    let uri = current.and_then(|t| t.cover.as_ref()).map(|c| c.uri());
    // «Обложка → Скорость» and «Пульсация → Сила».
    let period = if prefs.cover_spin && motion { prefs.cover_spin_s.max(1.0) } else { 0.0 };
    let spin = turn(ui, stage.id("np-cover-angle"), playing, bass, period, prefs.cover_ccw);
    let swell = if prefs.cover_pulse { 1.0 + prefs.cover_pulse_strength * live_kick } else { 1.0 };
    // Instant feedback: the cover sinks a little while pressed, so a tap
    // feels taken even while it waits to see whether a second one comes.
    // (The stage's copy is pressed by its scene, not the pointer.)
    let pressing = if stage.live {
        (prefs.cover_click_pause || prefs.dbl_click_shape > 0)
            && ctx.input(|i| {
                let inside = |p: Pos2| p.distance(cover_rect.center()) < cover_rect.width() * 0.45;
                i.pointer.primary_down()
                    && i.pointer.press_origin().is_some_and(inside)
                    && i.pointer.interact_pos().is_some_and(inside)
            })
    } else {
        stage.pressed
    };
    let press = anim::fraction(&ctx, stage.id("np-cover-press"), if pressing { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    let swell = swell * (1.0 - 0.04 * press);
    let shaped = Rect::from_center_size(cover_rect.center(), cover_rect.size() * swell);
    // No spin through the morph; the shape turns once it is reached.
    let toward = playing || !prefs.cover_pause_morph;
    let angle = cover_angle(ui, stage.id("np-cover-lobes"), shape, spin, morph, toward);
    let outline = match swap {
        Some((from, to, t)) => Outline::swap(from, to, t, shaped, angle),
        _ => Outline::new(shape, morph, shaped, angle),
    };
    let mega_outline = Outline::new(shape, morph, mega.cover, 0.0);
    let ring_scope = crate::prof::scope("np.ring");
    // Gone and settled: nothing of the ring runs (no frames asked for).
    if ring_k > 0.01 {
        // Against the cover, at its own pace; still with reduced motion.
        let period = if motion { nikgob_view::RING_SPIN_S } else { 0.0 };
        let angle = turn(ui, stage.id("np-ring-angle"), playing, bass, period, !prefs.cover_ccw);
        let heights = if bars { cava(ui, stage, &bands, playing && motion) } else { Vec::new() };
        let mut input = RingInput {
                style: prefs.ring_style,
                angle,
                transients: &transients,
                bars: &heights,
                room: pad,
                kick,
                // Every style follows the outline as it morphs.
                alpha: ring_k,
                shown: ring_k,
                base: normal_cover.width(),
                scale: 1.0,
                key: egui::Id::new("np-ring-dots"),
        };
        let alpha = input.alpha;
        input.alpha *= if theme.reduced_motion || !mega.wide { 1.0 - mega_k } else { 1.0 };
        ring(ui, &outline, &input);
        if theme.reduced_motion && mega.wide {
            input.alpha = alpha * mega_k;
            ring(ui, &mega_outline, &input);
        }
    }
    drop(ring_scope);
    let cover_scope = crate::prof::scope("np.cover");
    if theme.reduced_motion {
        let mut normal_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("np-cover-fade"));
        normal_ui.multiply_opacity(1.0 - mega_k);
        outline.paint(&normal_ui, uri.as_deref());
        let mut mega_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("mega-cover-fade"));
        mega_ui.multiply_opacity(mega_k);
        mega_outline.paint(&mega_ui, uri.as_deref());
    } else {
        outline.paint(ui, uri.as_deref());
    }
    drop(cover_scope);
    stage.part(&ctx, Part::Cover, cover_rect);
    stage.part(&ctx, Part::Ring, lerp_rect(cover_box, mega.cover.expand(RING_PAD), mega_k));
    let mut cover_resp = ui.interact(cover_rect, ui.id().with("np-cover"), Sense::click());
    // Only the shape itself is the cover: its square's corners count as
    // the background.
    let on_shape = |pos: Option<Pos2>| pos.is_some_and(|p| outline.contains(p));
    let hover_shape = cover_resp.hovered() && on_shape(cover_resp.hover_pos());
    if prefs.cover_click_pause && hover_shape {
        cover_resp = cover_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let shape_clicked = cover_resp.clicked() && on_shape(cover_resp.interact_pointer_pos());
    let corner_clicked = cover_resp.clicked() && !shape_clicked;
    if playing && !theme.reduced_motion {
        stage.repaint(&ctx);
    }

    let info_rect = snap(ui, info_rect);
    // Swallows clicks on the info column so they don't close the player.
    ui.interact(info_rect, ui.id().with("np-info-bg"), Sense::click());

    // Taps: a click on empty space closes at once, one on the cover shape
    // pauses; a second tap on the shape soon after changes the outline
    // instead. While that gesture is on, a tap on the shape waits only our
    // own double-tap window (`Prefs::np_tap_wait`, not egui's 0.3 s);
    // closing never waits. With no wait at all the tap pauses at once and
    // a second one undoes the pause as it changes the shape.
    let wait = f64::from(prefs.np_tap_wait.clamp(0.0, 0.4));
    let instant = wait < 0.05;
    let double_tap = if instant { 0.3 } else { wait };
    let gesture = match prefs.dbl_click_shape {
        0 => false,
        1 => prefs.cover_shape == CoverShape::Random,
        _ => prefs.cover_shape != CoverShape::Original,
    };
    let tap_id = stage.id("np-tap");
    if stage.live && (bg.clicked() || corner_clicked) {
        match outside_click(chrome.np_slot, embedded) {
            Outside::CloseInfo => chrome.np_slot = NpSlot::None,
            Outside::ClosePlayer => {
                ctx.data_mut(|d| d.remove::<(Option<Tap>, f64)>(tap_id));
                closed = true;
            }
            Outside::Nothing => {}
        }
    }
    let single = (prefs.cover_click_pause && shape_clicked).then_some(Tap::Pause);
    let fire = if !stage.live {
        None
    } else if gesture {
        let now = ctx.input(|i| i.time);
        // (what the first tap will do, when it landed).
        let pending = ctx.data(|d| d.get_temp::<(Option<Tap>, f64)>(tap_id));
        let mut now_fire = None;
        if shape_clicked {
            match pending {
                Some((done, at)) if now - at <= double_tap => {
                    ctx.data_mut(|d| d.remove::<(Option<Tap>, f64)>(tap_id));
                    change_shape(&ctx, prefs, out);
                    if instant {
                        // The first tap already paused: take it back.
                        now_fire = done;
                    }
                }
                _ => {
                    ctx.data_mut(|d| d.insert_temp(tap_id, (single, now)));
                    if instant {
                        now_fire = single;
                    }
                }
            }
        }
        match ctx.data(|d| d.get_temp::<(Option<Tap>, f64)>(tap_id)) {
            Some((tap, at)) if now - at > double_tap => {
                ctx.data_mut(|d| d.remove::<(Option<Tap>, f64)>(tap_id));
                // Left from before the player was closed: drop it.
                if instant { None } else { tap.filter(|_| now - at < double_tap + 0.25) }
            }
            Some((_, at)) => {
                stage.repaint_after(&ctx, std::time::Duration::from_secs_f64((at + double_tap - now).max(0.0) + 0.01));
                now_fire
            }
            None => now_fire,
        }
    } else {
        single
    };
    match fire {
        Some(Tap::Pause) => out.push(Command::TogglePlay),
        None => {}
    }
    let info_scope = crate::prof::scope("np.info");
    if mega_k < 0.999 {
        let mut normal_ui = ui.new_child(UiBuilder::new().max_rect(info_rect).id_salt("np-normal-info"));
        normal_ui.multiply_opacity(1.0 - mega_k);
        if slot_open { normal_ui.disable(); }
        info(&mut normal_ui, info_rect, wide, InfoMode::Normal, state, pages, chrome, stage, out);
    }
    if mega_k > 0.001 {
        let mut mega_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("np-mega-body"));
        mega_ui.multiply_opacity(mega_k);
        if !slot_open { mega_ui.disable(); }
        if mega.wide {
            info(&mut mega_ui, mega.info, false, InfoMode::Compact, state, pages, chrome, stage, out);
        } else {
            thumbnail_info(&mut mega_ui, mega.cover, state, chrome);
            info(&mut mega_ui, mega.info, false, InfoMode::Transport, state, pages, chrome, stage, out);
        }
        let slide = if theme.reduced_motion { 0.0 } else { 32.0 * (1.0 - mega_k) };
        let content = mega.content.translate(vec2(slide, 0.0));
        mega_ui.interact(content, mega_ui.id().with("np-content-bg"), Sense::click());
        if stage.live {
            chrome.wheel_block(content);
        }
        for (slot, alpha) in [NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info].into_iter().zip(content_alpha) {
            if alpha <= 0.001 { continue; }
            // The info lays itself out in columns over the whole width.
            let width = if matches!(slot, NpSlot::Lyrics | NpSlot::Info) { content.width() } else { content.width().min(720.0) };
            // The lyrics take the air above the content too, up to just
            // under the header (or the small cover's row).
            let lift = if slot == NpSlot::Lyrics { mega.lyrics_lift } else { 0.0 };
            let area = Rect::from_min_max(pos2(content.left(), content.top() - lift), pos2(content.left() + width, content.bottom()));
            let mut cui = mega_ui.new_child(UiBuilder::new().max_rect(area).id_salt(("np-content", slot as u8)));
            cui.set_clip_rect(area.intersect(ui.clip_rect()));
            cui.multiply_opacity(alpha);
            if chrome.np_slot != slot { cui.disable(); }
            match slot {
                NpSlot::Lyrics => {
                    let _p = crate::prof::scope("np.lyrics");
                    if super::lyrics::show_on(&mut cui, state, levels_bass, true, stage, out) && stage.live {
                        pages.open_setting_group(super::lyrics_ui::SETTINGS);
                        closed = true;
                    }
                }
                NpSlot::Queue => pane::queue(&mut cui, state, chrome, out),
                NpSlot::Info => {
                    if pane::info_sheet(&mut cui, state, out) && stage.live {
                        chrome.np_slot = NpSlot::None;
                    }
                }
                NpSlot::None => unreachable!(),
            }
        }
    }
    drop(info_scope);
    if state.prefs.np_volume_pop && !state.prefs.np_volume {
        volume_pop(ui, rect, state, chrome, stage, out);
    }

    closed && stage.live
}

/// What a click on Now Playing's empty space does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outside {
    /// The track's info is put away; the player stays.
    CloseInfo,
    ClosePlayer,
    Nothing,
}

/// A click beside the track's info (a look-up opened from the ⋮ menu) puts
/// it away, in the window and in mini mode alike; otherwise it closes the
/// player, which mini mode (`embedded`) has nothing under to close to.
fn outside_click(slot: NpSlot, embedded: bool) -> Outside {
    match (slot, embedded) {
        (NpSlot::Info, _) => Outside::CloseInfo,
        (_, false) => Outside::ClosePlayer,
        (_, true) => Outside::Nothing,
    }
}

/// Where snackbars go while Now Playing is open: `(area, bottom inset)`,
/// written each frame and read by the shell the next ([`snack_spot`]).
pub const SNACK_SPOT: &str = "np-snack-spot";

/// Picks the snackbars' place so they cover neither the transport nor the
/// cover: under the controls column (the cover beside it) or below
/// everything (the cover above), and where there is no room for that over
/// the cover's lower edge, on top of the picture, never of a control; with
/// a slot open, at the bottom.
fn snack_spot(ctx: &egui::Context, rect: Rect, wide: bool, slot_open: bool, cover: Rect, info: Rect) {
    // A two-line snackbar and its margins.
    const ROOM: f32 = 72.0 + 24.0 + 8.0;
    const INSET: f32 = 24.0;
    let room = rect.bottom() - info.bottom() >= ROOM;
    let over_cover = (rect.bottom() - cover.bottom() + 12.0).max(INSET);
    let spot = match (slot_open, wide, room) {
        (true, ..) => (rect, INSET),
        (false, true, true) => (Rect::from_x_y_ranges(info.x_range(), rect.y_range()), INSET),
        (false, true, false) => (Rect::from_x_y_ranges(cover.x_range(), rect.y_range()), over_cover),
        (false, false, true) => (rect, INSET),
        (false, false, false) => (rect, over_cover),
    };
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(SNACK_SPOT), spot));
}

/// «Громкость — при изменении»: a floating pill at the bottom centre that
/// shows the volume while it changes (wheel, keys, media keys, the menu)
/// and slips away `Prefs::np_volume_pop_hold` after the last change; it
/// stays while the pointer is on it, so it can be dragged too.
fn volume_pop(ui: &mut Ui, area: Rect, state: &ViewState, chrome: &mut Chrome, stage: &Stage, out: &mut Vec<Command>) {
    let hold = f64::from(state.prefs.np_volume_pop_hold.clamp(0.3, 5.0));
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let p = &state.player;
    let volume = if p.muted { 0.0 } else { chrome.volume_now(&ctx, state) };
    let now = ctx.input(|i| i.time);
    let id = stage.id("np-vol-pop");
    // (last volume, muted, when it last changed, pointer on the pill).
    let (last, muted, at, held) = ctx
        .data(|d| d.get_temp::<(f32, bool, f64, bool)>(id))
        .unwrap_or((volume, p.muted, f64::NEG_INFINITY, false));
    let at = if (last - volume).abs() > 1e-4 || muted != p.muted { now } else { at };
    let shown = held || now - at < hold;
    let k = anim::fraction(&ctx, id.with("k"), if shown { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    if shown && !held {
        stage.repaint_after(&ctx, std::time::Duration::from_secs_f64((at + hold - now).max(0.0) + 0.02));
    }
    let mut hovered = false;
    if k > 0.001 {
        let size = vec2(area.width().min(360.0) - 32.0, 64.0);
        let rise = if theme.reduced_motion { 0.0 } else { 28.0 * (1.0 - k) };
        let pill = Rect::from_center_size(
            pos2(area.center().x, area.bottom() - 24.0 - size.y * 0.5 + rise),
            size * (0.92 + 0.08 * k),
        );
        let radii = Radii::all(pill.height() * 0.5);
        let mut pui = ui.new_child(UiBuilder::new().max_rect(pill).id_salt("np-vol-pop-ui"));
        pui.multiply_opacity(k.clamp(0.0, 1.0));
        // Clicks on the pill must not close the player under it.
        let back = pui.interact(pill, id.with("bg"), Sense::click());
        paint::shadow(pui.painter(), pill, radii, crate::tokens::Elevation::Level3);
        paint::container(pui.painter(), pill, radii, s.surface_container_high, Stroke::NONE);
        let inner = pill.shrink2(vec2(12.0, 8.0));
        let mut row = pui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center))
                .id_salt("np-vol-pop-row"),
        );
        if row
            .add(IconButton::new(super::output::volume_icon(state, volume, p.muted)).tooltip(nikgob_core::i18n::player().sound_tip).id_salt("np-vol-pop-mute"))
            .clicked()
        {
            out.push(Command::ToggleMute);
        }
        let label_w = 44.0;
        let w = (row.available_width() - label_w - 8.0).max(64.0);
        let vo = Slider::new("np-vol-pop-slider", volume).track(12.0).width(w).show(&mut row);
        if let Some(v) = vo.dragging.or(vo.committed) {
            chrome.push_volume(state, v, out);
        }
        common::label(&mut row, &format!("{:.0}%", volume * 100.0), TypeRole::LabelLarge, s.on_surface);
        hovered = stage.live && (back.contains_pointer() || vo.dragging.is_some());
        if stage.live {
            chrome.volume_zone(pill, WheelZone::Np);
        }
        stage.part(&ctx, Part::Volume, pill);
    }
    ctx.data_mut(|d| d.insert_temp(id, (volume, p.muted, if hovered { now } else { at }, hovered)));
}

/// Retain the last content on close; only switching open slots crossfades it.
fn slot_opacities(ctx: &egui::Context, slot: NpSlot, stage: &Stage) -> [f32; 4] {
    let theme = Theme::get(ctx);
    let spring = anim::effective(ctx, theme.motion.fast_effects);
    let now = anim::now(ctx);
    let (alpha, settled) = ctx.data_mut(|d| {
        let target = match slot {
            NpSlot::None | NpSlot::Lyrics => [1.0, 0.0, 0.0, 0.0],
            NpSlot::Queue => [0.0, 1.0, 0.0, 0.0],
            NpSlot::Info => [0.0, 0.0, 1.0, 0.0],
        };
        let (last, a) = d.get_temp_mut_or_insert_with(stage.id("np-content-fade"), || (slot, Animated::new(target, spring)));
        if slot != NpSlot::None {
            if *last == NpSlot::None { a.snap(target); }
            if a.spring() != spring {
                a.animate_to(target, spring, now);
            } else {
                a.set_target(target, now);
            }
        }
        *last = slot;
        (a.value(now), a.is_settled(now))
    });
    // A crossfade finishes even on a still stage.
    if !settled { ctx.request_repaint(); }
    alpha.map(|a| a.clamp(0.0, 1.0))
}

/// Height of the text and controls block.
const INFO_H: f32 = TITLE_H + 4.0 + 24.0 + 24.0 + 64.0 + PROGRESS_GAP + CONTROLS_H + 16.0 + 48.0;
/// Air between the progress bar and the transport row.
const PROGRESS_GAP: f32 = 28.0;
const TITLE_H: f32 = 44.0;
const CONTROLS_H: f32 = 96.0;

const COMPACT_INFO_H: f32 = 32.0 + 4.0 + 24.0 + 16.0 + 64.0 + 16.0 + 56.0;
const TRANSPORT_H: f32 = 64.0 + 16.0 + 56.0;
/// How far the lyrics beside the cover end above the cover column's bottom
/// air: the control on their bottom edge (32 px) is then centred on the
/// transport row (56 px) where the column fills the height.
const LYRICS_CONTROL_LIFT: f32 = 12.0;
/// Air between the header (or the small cover's row) and the lyrics' top.
const LYRICS_TOP_GAP: f32 = 8.0;

struct MegaLayout {
    wide: bool,
    cover: Rect,
    info: Rect,
    content: Rect,
    /// How far above `content` the lyrics start: they span from just under
    /// the header (beside the cover) or the small cover's row (narrow) down
    /// to the control's row, their soft bands inside that span.
    lyrics_lift: f32,
}

/// Wide: the cover column and the content form one group centred in the
/// window, so a big screen has no empty half on the right.
fn mega_layout(body: Rect) -> MegaLayout {
    if body.width() >= 900.0 {
        // Apple Music style: the window splits in halves. The cover column
        // sits in the middle of the left half; the lyrics / queue start just
        // right of the centre line. Room at the window edges throughout.
        let pad = (body.width() * 0.05).clamp(24.0, 96.0);
        let mid = body.center().x;
        let half = mid - body.left() - pad;
        let col = (half - 24.0).clamp(300.0, 500.0).min(half);
        let left = body.left() + pad + (half - col) * 0.5;
        let gap = (body.width() * 0.03).clamp(24.0, 64.0);
        let content_left = mid + gap * 0.5;
        let lyrics_w = (body.right() - pad - content_left).min(860.0);
        // Air above and below, so the transport row never hugs the
        // window's bottom edge.
        let vpad = (body.height() * 0.06).clamp(24.0, 56.0);
        let cover = (col - 2.0 * RING_PAD)
            .min(body.height() - 2.0 * vpad - 2.0 * RING_PAD - 16.0 - COMPACT_INFO_H)
            .clamp(120.0, 440.0);
        let height = cover + 2.0 * RING_PAD + 16.0 + COMPACT_INFO_H;
        let top = body.center().y - height * 0.5;
        let info = Rect::from_min_size(pos2(left, top + cover + 2.0 * RING_PAD + 16.0), vec2(col, COMPACT_INFO_H));
        MegaLayout {
            wide: true,
            cover: Rect::from_center_size(pos2(left + col * 0.5, top + RING_PAD + cover * 0.5), vec2(cover, cover)),
            info,
            // The lyrics reach down to the cover column's bottom air, the
            // control on their bottom edge (they scroll on behind it):
            // level with the transport row where the column fills the
            // height, at that same bottom margin where it is centred
            // higher in a tall window.
            content: Rect::from_min_max(
                pos2(content_left, body.top()),
                pos2(content_left + lyrics_w, body.bottom() - vpad - LYRICS_CONTROL_LIFT),
            ),
            // `content` starts 16 px under the header.
            lyrics_lift: 16.0 - LYRICS_TOP_GAP,
        }
    } else {
        MegaLayout {
            wide: false,
            cover: Rect::from_min_size(body.min, vec2(56.0, 56.0)),
            info: Rect::from_min_size(pos2(body.left(), body.bottom() - TRANSPORT_H), vec2(body.width(), TRANSPORT_H)),
            content: Rect::from_min_max(pos2(body.left(), body.top() + 72.0), pos2(body.right(), body.bottom() - TRANSPORT_H - 16.0)),
            // `content` starts 16 px under the small cover's row.
            lyrics_lift: 16.0 - LYRICS_TOP_GAP,
        }
    }
}

fn lerp_rect(from: Rect, to: Rect, k: f32) -> Rect {
    Rect::from_min_max(from.min.lerp(to.min, k), from.max.lerp(to.max, k))
}

fn thumbnail_info(ui: &mut Ui, cover: Rect, state: &ViewState, chrome: &mut Chrome) {
    let Some(t) = &state.player.current else { return };
    let s = Theme::get(ui.ctx()).scheme;
    let rect = Rect::from_min_max(pos2(cover.right() + 16.0, cover.top()), pos2(ui.max_rect().right(), cover.bottom()));
    let mut tui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("mega-title"));
    tui.add(egui::Label::new(TypeRole::TitleLarge.emphasized().rich(&t.title, s.on_surface)).truncate());
    let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
    if let Some(r) = super::artist_links::label(&mut tui, "mega-artists", &t.artist, &t.artists, format) {
        chrome.nav = Some(r);
    }
}

/// The natural widths of «Artist · Album»: all the artists, the first
/// one's name, « · » and the album (0 without one).
#[derive(Clone, Copy)]
struct LineWidths {
    artist: f32,
    first: f32,
    sep: f32,
    album: f32,
}

/// The least of the album worth showing after « · »: a few letters and «…».
const ALBUM_MIN: f32 = 48.0;

/// How `width` is shared by the artists and « · album»: both whole when
/// they fit; else the album truncates down to [`ALBUM_MIN`], then the
/// artists down to the first one's name, then the album goes.
fn split_line(width: f32, w: LineWidths) -> (f32, Option<f32>) {
    if w.album <= 0.0 {
        return (w.artist.min(width), None);
    }
    let album = w.sep + w.album;
    if w.artist + album <= width {
        return (w.artist, Some(album));
    }
    let least = w.sep + ALBUM_MIN.min(w.album);
    if w.artist + least <= width {
        return (w.artist, Some(width - w.artist));
    }
    let artist = (width - least).max(w.first.min(w.artist)).min(width);
    let rest = width - artist;
    (artist, (rest >= least).then_some(rest))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InfoMode {
    Normal,
    Compact,
    Transport,
}

/// Title, artist · album, progress, transport and the action row (like,
/// add to playlist, volume, dislike).
#[allow(clippy::too_many_arguments)]
fn info(
    ui: &mut Ui,
    rect: Rect,
    wide: bool,
    mode: InfoMode,
    state: &ViewState,
    pages: &mut PageState,
    chrome: &mut Chrome,
    stage: &Stage,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let p = &state.player;
    // Another device's music while its link is lost is not shown as
    // playing; while its state comes late, the loading indicator.
    let playing = p.status == Status::Playing && !super::output::lost(state);
    let buffering = p.status == Status::Buffering || super::output::reconnecting(state);
    let current = p.current.as_ref();
    let col = rect;
    let mut y = col.top();
    // Of the two layouts crossfading, the one mostly shown has the parts.
    let mostly_shown = ui.opacity() > 0.5;
    let part = |part: Part, r: Rect| {
        if mostly_shown {
            stage.part(&ctx, part, r);
        }
    };

    let compact = mode != InfoMode::Normal;
    let title_h = if compact { 32.0 } else { TITLE_H };
    // Title and links.
    if mode != InfoMode::Transport {
        let text_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), title_h + 4.0 + 24.0));
        part(Part::Title, text_rect);
        let mut tui = ui.new_child(UiBuilder::new().max_rect(text_rect).id_salt("np-text"));
        tui.spacing_mut().item_spacing.y = 4.0;
        let title_role = if compact { TypeRole::HeadlineSmall } else if wide { TypeRole::DisplaySmall } else { TypeRole::HeadlineLarge };
        match current {
            Some(t) => {
                tui.add(egui::Label::new(title_role.emphasized().rich(&t.title, s.on_surface)).truncate());
                // The artists, « · » and the album are laid out alike, so
                // they share one baseline.
                tui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // «Artist · Album» in one line: the album gives way
                    // first, the artists keep at least the first one's name.
                    let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
                    let width_of = |text: &str| {
                        ui.painter()
                            .layout_job(egui::text::LayoutJob::single_section(text.to_owned(), format.clone()))
                            .size()
                            .x
                            .ceil()
                            + 1.0
                    };
                    let first = t.artists.0.first().map_or(t.artist.as_str(), |c| c.name.as_str());
                    let widths = LineWidths {
                        artist: width_of(&t.artist),
                        first: width_of(first),
                        sep: width_of(" · "),
                        album: t.album.as_deref().map_or(0.0, width_of),
                    };
                    // A track not played as recorded says so, quietly, at
                    // the end of the line (its room comes off the album).
                    let mark = speed_mark_of(state, t.id);
                    let mark = mark.as_deref();
                    let mark_w = mark.map_or(0.0, |m| speed_mark_width(ui, m) + SPEED_MARK_GAP);
                    let (artist_w, album_w) = split_line((ui.available_width() - mark_w).max(0.0), widths);
                    let h = ui.available_height();
                    // Each artist is a link of its own.
                    ui.allocate_ui(vec2(artist_w, h), |ui| {
                        if let Some(r) = super::artist_links::label(ui, "np-artists", &t.artist, &t.artists, format.clone()) {
                            chrome.nav = Some(r);
                        }
                    });
                    if let (Some(a), Some(w)) = (&t.album, album_w) {
                        crate::components::link_label(ui, "np-sep", " · ", &[], format.clone());
                        ui.allocate_ui(vec2(w - widths.sep, h), |ui| {
                            let album = crate::components::link_label(ui, "np-album", a, &[0..a.len()], format.clone());
                            if album.clicked.is_some() {
                                chrome.nav = t.release_id.map(nikgob_view::Route::Release);
                            }
                        });
                    }
                    if let Some(m) = mark {
                        ui.add_space(SPEED_MARK_GAP);
                        // The mark is the way to what it says.
                        if speed_mark(ui, m).clicked() {
                            sound::open(ui.ctx(), sound::Tab::Speed, None);
                        }
                    }
                });
            }
            None => {
                tui.label(title_role.style().rich(nikgob_core::i18n::player().nothing_playing, s.on_surface));
            }
        }
        y += title_h + 4.0 + 24.0 + if compact { 16.0 } else { 24.0 };
    }

    // Progress.
    let dur = p.duration.unwrap_or_default();
    let speed = p.speed;
    let prog_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), 64.0));
    part(Part::Progress, prog_rect);
    let mut pui = ui.new_child(UiBuilder::new().max_rect(prog_rect).id_salt("np-progress"));
    let so = Slider::new("np-slider", p.fraction_now())
        .width(col.width())
        .wavy(playing && state.prefs.progress_wave)
        .value_label(move |v| common::fmt_time(nikgob_view::heard_time(dur.mul_f32(v), speed)))
        .show(&mut pui);
    if let Some(v) = so.committed {
        out.push(Command::Seek(dur.mul_f32(v)));
    }
    let shown = so.dragging.map_or_else(|| p.position_now(), |v| dur.mul_f32(v));
    pui.horizontal(|ui| {
        let shown = nikgob_view::heard_time(shown, speed);
        ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(shown), s.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let heard = nikgob_view::heard_time(dur, speed);
            ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(heard), s.on_surface_variant));
            // What really plays, in one quiet line: where a stream's sound
            // comes from (a Spotify track may play from YouTube Music) and
            // its format, or a file's own format.
            if let Some(line) = common::sound_line(state) {
                ui.add_space(8.0);
                ui.label(TypeRole::LabelMedium.style().rich(line, s.on_surface_variant))
                    .on_hover_text(nikgob_core::i18n::player().sound_line_hint);
            }
            // With the content open there is no row for the output button:
            // another device's pill stands here, by the quality (the
            // settings' stage has none: its springs are the real one's).
            if compact && stage.live {
                let look = super::output::Look {
                    circle: 0.0,
                    pill: 22.0,
                    content: s.on_surface_variant,
                    accent: (s.primary, s.on_primary),
                    collapsed: false,
                };
                let wanted = state.target.is_some();
                if super::output::slot_width(ui, state, "np-compact-output", look, wanted) > 0.0 {
                    ui.add_space(8.0);
                }
                super::output::button(ui, state, "np-compact-output", look, wanted, chrome);
            }
        });
    });
    y += 64.0 + if compact { 16.0 } else { PROGRESS_GAP };

    // Controls: standard button group, hero play in the middle.
    // The largest sizes whose row fits the column (a narrow window steps
    // the side buttons down first, so play stays the biggest).
    // Shuffle and repeat only where Settings → Вид shows them.
    let buttons = state.prefs.player_buttons;
    let side = f32::from(2 + u8::from(buttons.shuffle) + u8::from(buttons.repeat));
    let row_w = |button: Size, hero: Size| {
        side * IconButton::new(Icon::Shuffle).size(button).natural_width()
            + IconButton::new(Icon::PlayArrow).size(hero).natural_width()
            + side * button.group_gap()
    };
    let tiers: &[(Size, Size)] = if compact {
        &[(Size::S, Size::M), (Size::Xs, Size::S)]
    } else {
        &[(Size::M, Size::L), (Size::S, Size::L), (Size::S, Size::M)]
    };
    // The compact layouts keep like and dislike at the transport row's
    // ends (under the progress bar's ends): the group leaves them room.
    let ends = if compact { 2.0 * (IconButton::new(Icon::Favorite).natural_width() + 8.0) } else { 0.0 };
    let (button_size, hero_size) = tiers
        .iter()
        .copied()
        .find(|&(b, h)| row_w(b, h) + ends <= col.width())
        .unwrap_or(tiers[tiers.len() - 1]);
    let group_w = row_w(button_size, hero_size);
    let controls = snap(
        ui,
        Rect::from_min_size(
            pos2(col.left() + ((col.width() - group_w) * 0.5).max(0.0), y),
            vec2(group_w.min(col.width()), hero_size.height()),
        ),
    );
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(controls)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-controls"),
    );
    let tonal = |icon: Icon| IconButton::new(icon).size(button_size).style(Style::Tonal);
    let repeat_icon = if p.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat };
    let hero = IconButton::new(Icon::PlayArrow)
        .selected_icon(Icon::Pause)
        .size(hero_size)
        .style(Style::Filled)
        .shape(BtnShape::Round)
        .selected(playing || buffering)
        .filled_icon(true)
        .colors(Colors {
            container: s.primary,
            content: s.on_primary,
            border: Color32::TRANSPARENT,
        })
        .tooltip(nikgob_core::i18n::player().play_tip);
    let mut items = Vec::with_capacity(5);
    // «Моя волна» turns to another cluster in the shuffle's slot.
    let wave = super::wave_plays(state);
    if buttons.shuffle && wave {
        items.push(Item::from(tonal(Icon::Refresh).tooltip(nikgob_core::i18n::player().wave_direction)));
    } else if buttons.shuffle {
        items.push(Item::from(tonal(Icon::Shuffle).selected(p.shuffle).tooltip(nikgob_core::i18n::common().shuffle)));
    }
    // Where «Назад» stands; play and «Далее» follow it.
    let prev = items.len();
    items.push(Item::from(tonal(Icon::SkipPrevious).tooltip(nikgob_core::i18n::player().prev_tooltip)));
    items.push(Item::from(hero));
    items.push(Item::from(tonal(Icon::SkipNext).tooltip(nikgob_core::i18n::player().next_tooltip)));
    if buttons.repeat {
        items.push(Item::from(
            tonal(repeat_icon)
                .selected(p.repeat != Repeat::Off)
                .tooltip(super::player_bar::repeat_tip(p.repeat)),
        ));
    }
    let r = ButtonGroup::new("np-group", button_size).show(&mut cui, items);
    if buttons.shuffle && r[0].clicked() {
        out.push(if wave { Command::OtherVibe } else { Command::ToggleShuffle });
    }
    if r[prev].clicked() {
        chrome::press_previous(&ctx, state, out);
    }
    if buffering {
        common::buffering_overlay(ui, r[prev + 1].rect, s.primary, s.on_primary);
    }
    if r[prev + 1].clicked() {
        out.push(Command::TogglePlay);
    }
    if r[prev + 2].clicked() {
        out.push(Command::Next);
    }
    if buttons.repeat && r[prev + 3].clicked() {
        out.push(Command::CycleRepeat);
    }
    if compact {
        // Like at the left end, dislike at the right, as in the full
        // layout's row under the transport; here level with the transport
        // (Now Playing with the lyrics, the queue or the details open).
        let Some(t) = current else { return };
        let ends = Rect::from_min_size(pos2(col.left(), controls.top()), vec2(col.width(), controls.height()));
        let mut lui = ui.new_child(
            UiBuilder::new().max_rect(ends).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("np-compact-like"),
        );
        let like_tip = format!("{} (Ctrl+L)", super::pages::like_label(t.liked));
        if buttons.like && lui.add(IconButton::new(Icon::Favorite).selected(t.liked).tooltip(like_tip).id_salt("np-like-compact")).clicked() {
            out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
        }
        if state.prefs.dislike_button {
            let disliked = state.dislikes.track(t.id);
            let mut dui = ui.new_child(
                UiBuilder::new().max_rect(ends).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt("np-compact-dislike"),
            );
            if dui.add(IconButton::new(Icon::ThumbDown).selected(disliked).tooltip(nikgob_core::i18n::player().dislike_tip).id_salt("np-dislike-compact")).clicked() {
                out.push(Command::Dislike { track: t.id, dislike: !disliked, sure: false, relike: false });
            }
        }
        return;
    }
    y += CONTROLS_H + 16.0;


    // Actions: like and add on the left; volume, the sound window and
    // dislike on the right, so like and dislike sit at opposite ends of
    // the row.
    let row = snap(ui, Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), 48.0)));
    let mut aui = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-actions"),
    );
    if let Some(t) = current {
        if buttons.like
            && aui
                .add(
                    IconButton::new(Icon::Favorite)
                        .selected(t.liked)
                        .tooltip(format!("{} (Ctrl+L)", super::pages::like_label(t.liked)))
                        .id_salt("np-like"),
                )
                .clicked()
        {
            out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
        }
        if buttons.add_to_playlist {
            let add = aui.add(IconButton::new(Icon::PlaylistAdd).tooltip(nikgob_core::i18n::player().add_playlist_tip).id_salt("np-add"));
            pages.playlist_menu(&add, vec![t.id]);
        }
    }
    // The sound of the track (its speed and the equalizer), by the volume.
    // Lit while either changes what is heard.
    let sound_changed =
        current.filter(|_| buttons.sound).map(|t| state.prefs.eq_on || state.speeds.of(t.id) != nikgob_view::SpeedVariant::Normal);
    let dislike = current.filter(|_| state.prefs.dislike_button).map(|t| (t.id, state.dislikes.track(t.id)));
    // «Где играть» left of the volume once there is another device; the
    // pill of another device's name while the window drives it.
    let output_look = super::output::Look {
        circle: 40.0,
        pill: 28.0,
        content: s.on_surface_variant,
        accent: (s.primary, s.on_primary),
        collapsed: false,
    };
    let output_on = super::output::available(state);
    let volume_row = aui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let volume = if p.muted { 0.0 } else { chrome.volume_now(&ctx, state) };
        let output_w = if stage.live { super::output::slot_width(ui, state, "np-output", output_look, output_on) } else { 0.0 };
        // The slider is optional; the wheel sets the volume either way.
        if state.prefs.np_volume {
            let buttons = 48.0 * (1.0 + f32::from(u8::from(dislike.is_some())) + f32::from(u8::from(sound_changed.is_some())))
                + if output_w > 0.0 { output_w + 8.0 } else { 0.0 };
            let w = (ui.available_width() - buttons).clamp(96.0, 220.0);
            let vo = Slider::new("np-vol", volume)
                .track(8.0)
                .width(w)
                .value_label(|v| format!("{:.0}%", v * 100.0))
                .show(ui);
            if let Some(v) = vo.dragging.or(vo.committed) {
                chrome.push_volume(state, v, out);
            }
        }
        if ui
            .add(
                IconButton::new(Icon::volume(volume, p.muted))
                    .tooltip(nikgob_core::i18n::player().volume_wheel_tip)
                    .id_salt("np-mute"),
            )
            .clicked()
        {
            out.push(Command::ToggleMute);
        }
        if stage.live {
            super::output::button(ui, state, "np-output", output_look, output_on, chrome);
        }
        if let Some(changed) = sound_changed {
            let button = ui.add(IconButton::new(Icon::Tune).selected(changed).tooltip(nikgob_core::i18n::screens().sound_window).id_salt("np-sound"));
            if button.clicked() {
                // The window grows out of its button.
                sound::open_from(ui.ctx(), sound::first_tab(state), None, button.rect);
            }
        }
        if let Some((track, disliked)) = dislike
            && ui
                .add(IconButton::new(Icon::ThumbDown).selected(disliked).tooltip(nikgob_core::i18n::player().dislike_tip).id_salt("np-dislike"))
                .clicked()
        {
            out.push(Command::Dislike { track, dislike: !disliked, sure: false, relike: false });
        }
    });
    part(Part::Volume, volume_row.response.rect);
}

/// Between the artist line and the speed mark.
const SPEED_MARK_GAP: f32 = 8.0;
/// Height of the speed mark and of the line it sits on.
const SPEED_MARK_H: f32 = 18.0;
const SPEED_LINE_H: f32 = 24.0;

/// The mark of the playing track's version («×1.2», «замедленно»), when it
/// is not played as recorded.
fn speed_mark_of(state: &ViewState, track: nikgob_view::TrackId) -> Option<std::borrow::Cow<'static, str>> {
    // A live track of another player plays as recorded whatever was chosen.
    if state.player.speed == 1.0 {
        return None;
    }
    state.speeds.of(track).chip()
}

fn speed_mark_width(ui: &Ui, text: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), Color32::WHITE);
    (galley.size().x + 14.0).ceil()
}

/// A quiet outlined mark: no fill, the colour of the line it ends. A press
/// on it opens the track's speed, so it takes the states of a button.
fn speed_mark(ui: &mut Ui, text: &str) -> egui::Response {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let galley = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), s.on_surface_variant);
    let size = vec2((galley.size().x + 14.0).ceil(), SPEED_LINE_H);
    let (line, response) = ui.allocate_exact_size(size, Sense::click());
    let rect = Rect::from_center_size(line.center(), vec2(size.x, SPEED_MARK_H));
    let layer = if response.is_pointer_button_down_on() {
        crate::tokens::state::PRESSED
    } else if response.hovered() {
        crate::tokens::state::HOVER
    } else {
        0.0
    };
    let layer = anim::fraction(ui.ctx(), response.id.with("layer"), layer, theme.motion.fast_effects);
    let p = ui.painter();
    paint::state_layer(p, rect, Radii::all(SPEED_MARK_H * 0.5), s.on_surface_variant, layer);
    p.rect_stroke(rect, SPEED_MARK_H * 0.5, Stroke::new(1.0, s.outline_variant), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() * 0.5, galley, s.on_surface_variant);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, nikgob_core::i18n::player::fmt_speed_mark(text)));
    crate::components::tooltip(response.on_hover_cursor(egui::CursorIcon::PointingHand), nikgob_core::i18n::player().speed_tip, Some("S"))
}

/// Background: soft colour clouds drifting over the window, breathing with

/// the bass in reactive mode (effects spring, no overshoot).
fn ambient(ui: &Ui, rect: Rect, state: &ViewState, levels_bass: f32, stage: &Stage) {
    let theme = Theme::get(ui.ctx());
    let mode = state.prefs.ambient;
    if mode == nikgob_view::Ambient::Off {
        return;
    }
    let playing = state.player.status == Status::Playing;
    let bass = if mode == nikgob_view::Ambient::Reactive && playing {
        levels_bass
    } else {
        0.0
    };
    let level = anim::fraction(ui.ctx(), stage.id("np-glow"), bass, theme.motion.fast_effects);
    // (The drift is shared with the real screen: one clock for both.)
    super::ambient::paint(ui.ctx(), ui.painter(), rect, level.clamp(0.0, 1.0), playing && stage.moving, state.prefs.ambient_strength);
}

#[cfg(test)]
mod tests;
