//! Now Playing (md3e-player.md §3.5): morphing cover, wavy progress, the
//! hero play button in a standard button group, the supporting pane, and the
//! ambient background (soft colour clouds following the bass).

use std::f32::consts::TAU;
use std::sync::OnceLock;

use egui::{Color32, Mesh, Rect, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_shapes::{CornerRounding, MaterialShape, Morph, Point, RoundedPolygon, paint as shape_paint};
use nikgob_view::{Command, LikeTarget, Repeat, Status, ViewState};

use super::{
    chrome::{self, Act, Chrome, Where},
    common,
    pages::PageState,
    pane,
};
use crate::{
    Icon, SizeClass, Theme, TypeRole,
    components::{
        BottomSheet, ButtonGroup, IconButton, Item, Size, Slider, Style, anim,
        button::{Colors, Shape as BtnShape},
        paint::{self, Radii},
    },
    size_class,
    tokens::shape,
};

/// Cookie12Sided (playing) ↔ rounded square with radius 48/480 (paused).
fn cover_morph() -> &'static Morph {
    static M: OnceLock<Morph> = OnceLock::new();
    M.get_or_init(|| {
        let square = RoundedPolygon::rectangle(
            1.0,
            1.0,
            CornerRounding::new(0.1, 0.0),
            None,
            Point::new(0.5, 0.5),
        )
        .expect("unit square")
        .normalized();
        Morph::new(&square, MaterialShape::Cookie12Sided.polygon())
    })
}

/// Draws the cover texture cropped to the morph at `progress`, rotated with it.
pub fn shaped_cover(ui: &Ui, uri: Option<&str>, rect: Rect, progress: f32, rotation: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    let cubics = cover_morph().as_cubics(progress.clamp(0.0, 1.0));
    let texture = uri.and_then(|u| {
        egui::Image::new(u.to_owned())
            .load_for_size(ui.ctx(), rect.size())
            .ok()
            .and_then(|p| p.texture_id())
    });
    let opts = shape_paint::FillOptions::for_ppp(ui.ctx().pixels_per_point());
    let mut mesh: Mesh = match texture {
        Some(tex) => shape_paint::texture_mesh(
            &cubics,
            rect,
            0.0,
            tex,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
            &opts,
        ),
        None => shape_paint::fill_mesh(&cubics, rect, 0.0, s.secondary_container, &opts),
    };
    // Rotate geometry (UVs stay), so the picture turns with its frame.
    if rotation != 0.0 {
        let c = rect.center();
        let (sin, cos) = rotation.sin_cos();
        for v in &mut mesh.vertices {
            let d = v.pos - c;
            v.pos = c + vec2(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
        }
    }
    if !mesh.is_empty() {
        ui.painter().add(Shape::mesh(mesh));
    }
    if texture.is_none() {
        let icon = rect.width() * 0.3;
        Icon::MusicNote
            .widget(false, s.on_secondary_container, icon)
            .paint_at(ui, Rect::from_center_size(rect.center(), vec2(icon, icon)));
    }
}

/// Draws Now Playing in `rect`. `k` is the container-transform progress
/// (0 = still the floating pill, 1 = full screen). `embedded` = mini mode,
/// with no close button. Returns true when the user closes it (the close
/// button, a swipe down, or a navigation chosen from a menu).
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    state: &ViewState,
    k: f32,
    embedded: bool,
    pages: &mut PageState,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let p = &state.player;
    let playing = p.status == Status::Playing;
    let mut closed = false;

    // Container: the pill's colour and radius turn into the full surface.
    let kc = k.clamp(0.0, 1.0);
    let radius = shape::FULL.min(rect.height() * 0.5) * (1.0 - kc);
    let bg = crate::lerp_color(s.primary_container, s.surface, kc);
    paint::container(ui.painter(), rect, Radii::all(radius), bg, Stroke::NONE);
    // Swallow input under the overlay; a swipe down closes it.
    let bg = ui.interact(rect, ui.id().with("np-bg"), Sense::click_and_drag());
    if !embedded
        && kc > 0.9
        && bg.drag_stopped()
        && let Some(d) = bg.total_drag_delta()
        && d.y > 96.0
        && d.y > d.x.abs() * 1.5
    {
        closed = true;
    }
    if kc < 0.35 {
        return closed;
    }
    let content_alpha = ((kc - 0.35) / 0.65).clamp(0.0, 1.0);
    ui.multiply_opacity(content_alpha);

    ambient(ui, rect, state, pages);

    let class = size_class(rect.width());
    let margin = class.margin();
    let inner = rect.shrink2(vec2(margin, 16.0));

    // Header.
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), 48.0));
    let mut hui = ui.new_child(
        UiBuilder::new()
            .max_rect(header)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if !embedded
        && hui
            .add(IconButton::new(Icon::ExpandMore).tooltip("Свернуть (Esc)").id_salt("np-close"))
            .clicked()
    {
        closed = true;
    }
    let ctx_text = p
        .context
        .as_deref()
        .map_or_else(|| "Сейчас играет".to_owned(), |c| format!("Играет из «{c}»"));
    let two_col = class >= SizeClass::Expanded;
    hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = ui.add(IconButton::new(Icon::MoreVert).tooltip("Ещё").id_salt("np-more"));
        if let Some(t) = p.current.as_ref() {
            let acts = [
                Act::Playlist,
                Act::Artist,
                Act::Album,
                Act::Info,
                Act::Tags,
                Act::Replace,
                Act::Visualizer,
                Act::Sep,
                Act::Trash,
            ];
            let wh = Where::Np { tab: &mut pages.np_tab };
            chrome.track_menu(&more, "np-more", &acts, t, state, wh, out);
        }
        // Queue, lyrics and info: a side column when it fits, a bottom
        // sheet otherwise; hidden until asked for.
        if ui
                .add(
                    IconButton::new(Icon::QueueMusic)
                        .style(Style::Tonal)
                        .selected(chrome.np_sheet)
                        .tooltip("Очередь, текст, инфо")
                        .id_salt("np-sheet-btn"),
                )
                .clicked()
        {
            chrome.np_sheet = !chrome.np_sheet;
        }
        ui.with_layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
            common::label(ui, &ctx_text, TypeRole::TitleSmall, s.on_surface_variant);
        });
    });

    // Columns: the pane slides in from the right edge when opened.
    let body = Rect::from_min_max(pos2(inner.left(), header.bottom() + 16.0), inner.max);
    let pane_full = if class >= SizeClass::ExtraLarge { 412.0 } else { 360.0 };
    let pane_k = anim::fraction(
        &ctx,
        egui::Id::new("np-pane-open"),
        if two_col && chrome.np_sheet { 1.0 } else { 0.0 },
        theme.motion.default_spatial,
    )
    .clamp(0.0, 1.0);
    let pane_w = (pane_full + 24.0) * pane_k;
    let left = Rect::from_min_max(body.min, pos2(body.right() - pane_w, body.bottom()));
    if two_col && pane_k > 0.01 {
        let pr = Rect::from_min_size(
            pos2(body.right() - pane_w + 24.0 * pane_k, body.top()),
            vec2(pane_full, body.height()),
        );
        let clip = Rect::from_min_max(pr.min, pos2(rect.right(), pr.bottom()));
        let painter = ui.painter().with_clip_rect(clip);
        painter.rect_filled(pr, shape::EXTRA_LARGE as u8, s.surface_container_low.gamma_multiply(0.85 * pane_k));
        let mut pui = ui.new_child(UiBuilder::new().max_rect(pr.shrink(16.0)).id_salt("np-pane"));
        pui.set_clip_rect(clip.intersect(pr));
        pui.multiply_opacity(pane_k);
        pane::show(&mut pui, state, &mut pages.np_tab, chrome, out);
    }

    // Left column, vertically centred block: cover, text, progress, controls, volume.
    let controls_h = 96.0;
    let text_h = 40.0 + 24.0 + 8.0;
    // text, gap, progress (slider + labels), gap, controls, gap, volume.
    let rest_h = text_h + 8.0 + 64.0 + 24.0 + controls_h + 24.0 + 44.0;
    let cover = (left.height() - rest_h - 24.0).min(left.width() - 32.0).clamp(120.0, 480.0);
    let block_h = cover + 24.0 + rest_h;
    let top = left.top() + ((left.height() - block_h) * 0.5).max(0.0);
    let col_w = cover.max(360.0).min(left.width());
    let col = Rect::from_min_size(pos2(left.center().x - col_w * 0.5, top), vec2(col_w, block_h));

    // Cover morph: playing → Cookie12Sided turning slowly; paused → square.
    let morph = anim::fraction(
        &ctx,
        egui::Id::new("np-cover-morph"),
        if playing { 1.0 } else { 0.0 },
        theme.motion.default_spatial,
    );
    let rotation = if theme.reduced_motion {
        0.0
    } else {
        // From the track position, so it never jumps or restarts.
        p.position_now().as_secs_f32() / 40.0 * TAU * morph.clamp(0.0, 1.0)
    };
    let cover_rect = Rect::from_center_size(pos2(col.center().x, col.top() + cover * 0.5), vec2(cover, cover));
    let current = p.current.as_ref();
    let uri = current.and_then(|t| t.cover.as_ref()).map(|c| c.uri());
    shaped_cover(ui, uri.as_deref(), cover_rect, morph, rotation);
    if playing && !theme.reduced_motion {
        ctx.request_repaint();
    }

    let mut y = cover_rect.bottom() + 24.0;
    let text_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), text_h));
    let mut tui = ui.new_child(UiBuilder::new().max_rect(text_rect).id_salt("np-text"));
    tui.spacing_mut().item_spacing.y = 4.0;
    tui.horizontal(|ui| {
        let w = ui.available_width() - 3.0 * 48.0;
        ui.vertical(|ui| {
            ui.set_width(w.max(100.0));
            match current {
                Some(t) => {
                    ui.add(
                        egui::Label::new(
                            TypeRole::HeadlineLarge.emphasized().rich(&t.title, s.on_surface),
                        )
                        .truncate(),
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let link = |ui: &mut Ui, text: &str| {
                            let r = ui.add(
                                egui::Label::new(
                                    TypeRole::TitleMedium.style().rich(text, s.on_surface_variant),
                                )
                                .truncate()
                                .sense(Sense::click()),
                            );
                            r.on_hover_cursor(egui::CursorIcon::PointingHand)
                        };
                        if link(ui, &t.artist).clicked() {
                            chrome.nav = t.artist_id.map(nikgob_view::Route::Artist);
                        }
                        if let Some(a) = &t.album {
                            ui.label(
                                TypeRole::TitleMedium.style().rich(" · ", s.on_surface_variant),
                            );
                            if link(ui, a).clicked() {
                                chrome.nav = t.release_id.map(nikgob_view::Route::Release);
                            }
                        }
                    });
                }
                None => {
                    ui.label(TypeRole::HeadlineLarge.style().rich("Ничего не играет", s.on_surface));
                }
            }
        });
        if let Some(t) = current {
            if ui
                .add(
                    IconButton::new(Icon::Favorite)
                        .selected(t.liked)
                        .tooltip("Нравится (Ctrl+L)")
                        .id_salt("np-like"),
                )
                .clicked()
            {
                out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
            }
            let add = ui.add(IconButton::new(Icon::PlaylistAdd).tooltip("В плейлист").id_salt("np-add"));
            pages.playlist_menu(ui, &add, vec![t.id], state, out);
        }
    });
    y += text_h;

    // Progress.
    y += 8.0;
    let dur = p.duration.unwrap_or_default();
    let prog_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), 44.0 + 20.0));
    let mut pui = ui.new_child(UiBuilder::new().max_rect(prog_rect).id_salt("np-progress"));
    let so = Slider::new("np-slider", p.fraction_now())
        .width(col.width())
        .wavy(playing)
        .value_label(move |v| common::fmt_time(dur.mul_f32(v)))
        .show(&mut pui);
    if let Some(v) = so.committed {
        out.push(Command::Seek(dur.mul_f32(v)));
    }
    let shown = so.dragging.map_or_else(|| p.position_now(), |v| dur.mul_f32(v));
    pui.horizontal(|ui| {
        ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(shown), s.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(dur), s.on_surface_variant));
            if let Some(q) = current.and_then(|t| t.quality.as_deref()) {
                ui.add_space(8.0);
                ui.label(TypeRole::LabelMedium.style().rich(q, s.primary));
            }
        });
    });
    y += 44.0 + 20.0 + 24.0;

    // Controls: standard button group, hero play in the middle.
    let controls = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), controls_h));
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(controls)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-controls"),
    );
    let group_w = 4.0 * 56.0 + 96.0 + 4.0 * 8.0 + 2.0 * 16.0;
    cui.add_space(((controls.width() - group_w) * 0.5).max(0.0));
    let tonal = |icon: Icon| IconButton::new(icon).size(Size::M).style(Style::Tonal);
    let repeat_icon = if p.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat };
    let hero = IconButton::new(Icon::PlayArrow)
        .selected_icon(Icon::Pause)
        .size(Size::L)
        .style(Style::Filled)
        .shape(BtnShape::Round)
        .selected(playing)
        .filled_icon(true)
        .colors(Colors {
            container: s.primary,
            content: s.on_primary,
            border: Color32::TRANSPARENT,
        })
        .tooltip("Играть / пауза (Space)");
    let r = ButtonGroup::new("np-group", Size::M).show(
        &mut cui,
        vec![
            Item::from(tonal(Icon::Shuffle).selected(p.shuffle).tooltip("Перемешать")),
            Item::from(tonal(Icon::SkipPrevious).tooltip("Назад (Ctrl+←)")),
            Item::from(hero),
            Item::from(tonal(Icon::SkipNext).tooltip("Далее (Ctrl+→)")),
            Item::from(
                tonal(repeat_icon)
                    .selected(p.repeat != Repeat::Off)
                    .tooltip("Повтор: выкл → всё → один"),
            ),
        ],
    );
    if r[0].clicked() {
        out.push(Command::ToggleShuffle);
    }
    if r[1].clicked() {
        chrome::press_previous(&ctx, state, out);
    }
    if r[2].clicked() {
        out.push(Command::TogglePlay);
    }
    if r[3].clicked() {
        out.push(Command::Next);
    }
    if r[4].clicked() {
        out.push(Command::CycleRepeat);
    }
    y += controls_h + 24.0;

    // Volume.
    let vol_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width().min(320.0), 44.0));
    let mut vui = ui.new_child(
        UiBuilder::new()
            .max_rect(vol_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-volume"),
    );
    if vui
        .add(
            IconButton::new(if p.muted || p.volume == 0.0 { Icon::VolumeOff } else { Icon::VolumeUp })
                .tooltip("Звук")
                .id_salt("np-mute"),
        )
        .clicked()
    {
        out.push(Command::ToggleMute);
    }
    let w = vui.available_width();
    let vo = Slider::new("np-vol", if p.muted { 0.0 } else { p.volume })
        .track(8.0)
        .width(w)
        .value_label(|v| format!("{:.0}%", v * 100.0))
        .show(&mut vui);
    if let Some(v) = vo.dragging.or(vo.committed) {
        out.push(Command::SetVolume(v));
    }

    // Single column: the pane lives in a bottom sheet.
    if !two_col {
        let mut open = chrome.np_sheet;
        BottomSheet::new("np-sheet").show(&ctx, &mut open, |ui| {
            pane::show(ui, state, &mut pages.np_tab, chrome, out);
        });
        chrome.np_sheet = open;
    }

    closed
}

/// Background: soft colour clouds drifting over the window, breathing with
/// the bass in reactive mode (effects spring, no overshoot).
fn ambient(ui: &Ui, rect: Rect, state: &ViewState, pages: &PageState) {
    let theme = Theme::get(ui.ctx());
    let mode = state.prefs.ambient;
    if mode == nikgob_view::Ambient::Off {
        return;
    }
    let playing = state.player.status == Status::Playing;
    let bass = if mode == nikgob_view::Ambient::Reactive && playing {
        pages.levels_bass
    } else {
        0.0
    };
    let level = anim::fraction(ui.ctx(), egui::Id::new("np-glow"), bass, theme.motion.fast_effects);
    super::ambient::paint(ui.ctx(), ui.painter(), rect, level.clamp(0.0, 1.0), playing);
}
