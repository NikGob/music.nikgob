//! The tag editor (md3e-player.md §4): a dialog up to 560 wide, full screen
//! on Compact, with outlined 56 dp fields, a MusicBrainz lookup and its
//! suggestions. Drawn from [`super::Md3eUi`] while `state.tag_editor.track`
//! is set.

use egui::{
    Align, Align2, Area, Context, Frame, Id, Key, Layout, Margin, Modifiers, Order, Sense,
    Stroke, Ui, emath::TSTransform, vec2,
};
use nikgob_view::{Command, TagEdit, TrackId, ViewState};

use crate::{
    Elevation, Icon, SizeClass, Theme, TypeRole,
    components::{
        Button, IconButton, ListItem, Size, Style, TextField, LoadingIndicator, anim,
        list::Leading,
    },
    size_class,
};

/// Fields being edited; reset whenever the runtime reads new values.
#[derive(Default)]
pub struct TagEditorUi {
    fields: TagEdit,
    track: Option<TrackId>,
    seen: TagEdit,
    first_frame: bool,
}

impl TagEditorUi {
    pub fn show(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let ed = &state.tag_editor;
        let Some(track) = ed.track else {
            self.track = None;
            return;
        };
        let theme = Theme::get(ctx);
        let s = theme.scheme;
        let fade = Id::new("md3e-tag-editor-fade");
        if self.track != Some(track) {
            self.track = Some(track);
            self.first_frame = true;
            self.fields = ed.current.clone();
            self.seen = ed.current.clone();
            // Seed the fade at 0 so the dialog animates in.
            anim::fraction(ctx, fade, 0.0, theme.motion.fast_effects);
            anim::snap_f32(ctx, fade, 0.0);
        } else if self.seen != ed.current {
            self.fields = ed.current.clone();
            self.seen = ed.current.clone();
        }
        let alpha = anim::fraction(ctx, fade, 1.0, theme.motion.fast_effects).clamp(0.0, 1.0);
        let spatial = anim::fraction(ctx, fade.with("s"), 1.0, theme.motion.fast_spatial);

        let screen = ctx.content_rect();
        let compact = size_class(screen.width()) == SizeClass::Compact;
        let mut close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let mut save = false;
        let mut lookup = false;
        let mut picked: Option<TagEdit> = None;

        // Scrim: swallows input; a click does not discard the edits.
        Area::new(fade.with("scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                let c = s.scrim_overlay().gamma_multiply(alpha);
                ui.painter().rect_filled(rect, 0.0, c);
            });

        let first = std::mem::take(&mut self.first_frame);
        let loading = ed.loading;
        let bg = if compact { s.surface } else { s.surface_container_high };
        let layer = egui::LayerId::new(Order::Tooltip, fade.with("dialog"));
        let area = Area::new(fade.with("dialog"))
            .order(Order::Tooltip)
            .fixed_pos(if compact { screen.min } else { screen.center() })
            .pivot(if compact { Align2::LEFT_TOP } else { Align2::CENTER_CENTER })
            .constrain(false)
            .interactable(true);
        let resp = area.show(ctx, |ui| {
            ui.set_opacity(alpha);
            let (width, margin, radius, shadow) = if compact {
                (screen.width(), 16, 0, Elevation::Level0)
            } else {
                ((screen.width() - 48.0).clamp(280.0, 560.0), 24, 28, Elevation::Level3)
            };
            let frame = Frame {
                inner_margin: Margin::same(margin),
                outer_margin: Margin::ZERO,
                corner_radius: egui::CornerRadius::same(radius),
                shadow: shadow.shadow(),
                fill: bg,
                stroke: Stroke::NONE,
            };
            frame.show(ui, |ui| {
                let inner_w = width - 2.0 * f32::from(margin);
                ui.set_width(inner_w);
                ui.spacing_mut().item_spacing.y = 0.0;
                let body_h = if compact {
                    screen.height() - 2.0 * f32::from(margin) - 56.0 - 16.0
                } else {
                    // header + gaps + actions of the dialog around the body
                    (screen.height() - 48.0 - 48.0 - 40.0 - 24.0 - 24.0 - 32.0).max(160.0)
                };
                if compact {
                    ui.allocate_ui_with_layout(
                        vec2(inner_w, 56.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            if ui
                                .add(IconButton::new(Icon::Close).tooltip("Закрыть (Esc)").id_salt("tag-close"))
                                .clicked()
                            {
                                close = true;
                            }
                            ui.label(TypeRole::TitleLarge.style().rich("Теги", s.on_surface));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ok = !self.fields.title.trim().is_empty() && !loading;
                                if ui
                                    .add(Button::new("Сохранить").size(Size::S).enabled(ok).id_salt("tag-save"))
                                    .clicked()
                                {
                                    save = true;
                                }
                            });
                        },
                    );
                    ui.add_space(16.0);
                } else {
                    ui.label(TypeRole::HeadlineSmall.style().rich("Теги", s.on_surface));
                    ui.add_space(16.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("tag-scroll")
                    .max_height(body_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        self.body(ui, ed, bg, first, &mut lookup, &mut picked);
                    });
                if !compact {
                    ui.add_space(24.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let ok = !self.fields.title.trim().is_empty() && !loading;
                        if ui
                            .add(Button::new("Сохранить").size(Size::S).enabled(ok).id_salt("tag-save"))
                            .clicked()
                        {
                            save = true;
                        }
                        if ui
                            .add(Button::new("Отмена").style(Style::Text).size(Size::S).id_salt("tag-cancel"))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                }
            });
        });
        if !compact {
            let k = 0.85 + 0.15 * spatial;
            let pivot = screen.center().to_vec2();
            ctx.set_transform_layer(layer, TSTransform::new(pivot * (1.0 - k), k));
        } else {
            ctx.set_transform_layer(layer, TSTransform::IDENTITY);
        }
        let _ = resp;

        if lookup {
            out.push(Command::LookupTags { track, query: self.fields.clone() });
        }
        if let Some(t) = picked {
            self.fields = t;
        }
        if save {
            out.push(Command::SaveTags { track, tags: self.fields.clone() });
        }
        if close {
            out.push(Command::CloseTagEditor);
        }
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        ed: &nikgob_view::TagEditor,
        bg: egui::Color32,
        first: bool,
        lookup: &mut bool,
        picked: &mut Option<TagEdit>,
    ) {
        let s = Theme::get(ui.ctx()).scheme;
        let enabled = !ed.loading;
        let f = &mut self.fields;
        let field = |ui: &mut Ui, text: &mut String, label: &str, salt: &'static str, focus: bool| {
            ui.add_space(12.0);
            // A scope, because the text editor inside a field moves the
            // parent's cursor back up by itself.
            ui.scope(|ui| {
                TextField::new(text, label)
                    .outlined()
                    .background(bg)
                    .enabled(enabled)
                    .request_focus(focus)
                    .id_salt(salt)
                    .show(ui)
            });
        };
        ui.add_space(8.0);
        if ed.loading {
            ui.vertical_centered(|ui| {
                ui.add(LoadingIndicator::new());
            });
        }
        field(ui, &mut f.title, "Название", "tag-title", first);
        field(ui, &mut f.artist, "Исполнитель", "tag-artist", false);
        field(ui, &mut f.album, "Альбом", "tag-album", false);
        field(ui, &mut f.album_artist, "Исполнитель альбома", "tag-album-artist", false);
        ui.add_space(12.0);
        let w = ((ui.available_width() - 24.0) / 3.0).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            for (text, label, salt) in [
                (&mut f.year, "Год", "tag-year"),
                (&mut f.track_no, "Трек №", "tag-track"),
                (&mut f.disc_no, "Диск №", "tag-disc"),
            ] {
                ui.scope(|ui| {
                    TextField::new(text, label)
                        .outlined()
                        .background(bg)
                        .enabled(enabled)
                        .width(w)
                        .id_salt(salt)
                        .show(ui)
                });
            }
        });
        field(ui, &mut f.genre, "Жанр", "tag-genre", false);

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    Button::new("Найти в MusicBrainz")
                        .icon(Icon::Search)
                        .style(Style::Outlined)
                        .size(Size::S)
                        .enabled(!ed.looking_up && enabled)
                        .id_salt("tag-lookup"),
                )
                .clicked()
            {
                *lookup = true;
            }
            if ed.looking_up {
                ui.add_space(8.0);
                ui.add(LoadingIndicator::new().size(40.0));
            }
        });
        if let Some(e) = &ed.error {
            ui.add_space(12.0);
            ui.add(
                egui::Label::new(TypeRole::BodyMedium.style().rich(e, s.error)).wrap(),
            );
        }
        if !ed.suggestions.is_empty() {
            ui.add_space(20.0);
            ui.label(TypeRole::TitleSmall.style().rich("Варианты из MusicBrainz", s.on_surface_variant));
            ui.add_space(8.0);
            let n = ed.suggestions.len();
            let rows = crate::components::segmented_list(ui, n, |i| {
                let (label, t) = &ed.suggestions[i];
                let mut line = t.artist.clone();
                if !t.album.is_empty() {
                    line = format!("{line} — {}", t.album);
                }
                if !t.year.is_empty() {
                    line = format!("{line} ({})", t.year);
                }
                ListItem::new(label.clone())
                    .supporting(line)
                    .leading(Leading::Icon(Icon::Album))
                    .id_salt(("tag-suggestion", i))
            });
            for (i, r) in rows.iter().enumerate() {
                if r.response.clicked() {
                    *picked = Some(ed.suggestions[i].1.clone());
                }
            }
        }
        ui.add_space(8.0);
    }
}
