//! «Загрузки»: where «Хочу скачать» looks and in what
//! order, what a like downloads, the quality, Soulseek, the torrents'
//! Prowlarr and the seeding.

use egui::{Ui, emath::TSTransform, vec2};
use nikgob_core::i18n;
use nikgob_view::{self as v, Command, Prefs, ViewState};

use super::super::PageState;
use super::super::settings_help::{self, SettingId as Id};
use super::preset;
use super::rows::{self, Note, Num, child, dimmed, group, block, menu, row, row_note, segments, switch};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, IconButton, LoadingIndicator, Size, Style, Switch, TextField, anim},
};

pub(super) fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    group(ui, st.g_sources);
    // Обычно · Всё · Свои: the one under the pointer tints the
    // sources it would switch and shows where their switches would go.
    let mut preview = preset::row(ui, v::presets::Area::Sources, p);
    let saved = p.clone();
    download_sources(ui, state, preview.as_mut().unwrap_or(&mut *p), &saved);

    group(ui, st.g_on_like);
    segments(ui, Id::LikeDownload, p);

    group(ui, st.g_quality);
    let never = p.upgrade_every == v::UpgradeEvery::Never;
    dimmed(ui, never, |ui| segments(ui, Id::UpgradeBar, p));
    menu(ui, Id::UpgradeEvery, p);
    segments(ui, Id::OpusBitrate, p);
    let never = p.upgrade_every == v::UpgradeEvery::Never;
    dimmed(ui, never, |ui| segments(ui, Id::UpgradeAuto, p));
    segments(ui, Id::YoutubeFallback, p);
    menu(ui, Id::FakeFlac, p);
    // TODO(lead): the concept moves this to the Корзина page (library.rs);
    // it stays here until that page has it, so the undo is never lost.
    row(ui, Id::FakeRestore, |ui| {
        if ui.add(Button::new(st.restore).style(Style::Outlined).id_salt("set-fake-restore")).clicked() {
            out.push(Command::RestoreConvertedFlac);
        }
    });

    group(ui, "Soulseek");
    soulseek(ui, state, pages, out);

    group(ui, st.g_torrents);
    prowlarr(ui, state, p, pages, out);

    group(ui, st.g_seeding);
    // Не раздавать · Обычно · Щедро · Свои, with the rows they own.
    let mut preview = preset::row(ui, v::presets::Area::Seeding, p);
    {
        let p: &mut Prefs = preview.as_mut().unwrap_or(&mut *p);
        switch(ui, Id::NoSeed, &mut p.no_seed);
        let no_seed = p.no_seed;
        child(ui, no_seed, |ui| {
            if let Some(v) = rows::slider(ui, Id::SeedLimit, p.seed_limit_gb as f32, Num::new(0.0, 500.0, 5.0, 0, st.suffix_gb).zero(st.no_limit)) {
                p.seed_limit_gb = v as u32;
            }
            if let Some(v) = rows::slider(ui, Id::SeedDays, p.seed_days as f32, Num::new(1.0, 90.0, 1.0, 0, "").unit(st.days)) {
                p.seed_days = v as u32;
            }
            if let Some(v) = rows::slider(ui, Id::SeedRatio, p.seed_ratio, Num::new(0.0, 5.0, 0.5, 1, "").zero(st.no_cap)) {
                p.seed_ratio = v;
            }
        });
    }
    let (dragged, set) = rows::speed(ui, Id::DownloadLimit, p.download_limit_kbps, true);
    if let Some(v) = set.or(dragged) {
        p.download_limit_kbps = v;
    }
    let (dragged, set) = rows::speed(ui, Id::UploadLimit, p.upload_limit_kbps, true);
    if let Some(v) = set.or(dragged) {
        p.upload_limit_kbps = v;
    }
    soulseek_upload(ui, state, out);
}

/// «Откуда качать»: where «Хочу скачать» looks, in the
/// order it prefers them at one quality, each with its switch and «Выше» /
/// «Ниже». Lossless comes first whichever source has it; a source the app
/// does not have is left out (torrents and Soulseek stay: they say what
/// they need, dimmed until set up). A move slides the rows to their new
/// places.
/// `saved`: the prefs as they are (`p` may be a preset's preview): what a
/// row says under its name follows them, so a preset under the pointer
/// changes the switches only and no row grows or shrinks under it.
fn download_sources(ui: &mut Ui, state: &ViewState, p: &mut Prefs, saved: &Prefs) {
    let st = i18n::settings_tabs();
    let mut order = p.source_order();
    let shown: Vec<v::SourceKey> = order
        .iter()
        .map(|o| o.key)
        .filter(|k| {
            let needs_setup = matches!(k, v::SourceKey::Torrents | v::SourceKey::Soulseek);
            state.downloads.sources.iter().any(|v| v.key == *k && (v.available || needs_setup))
        })
        .collect();
    let mut moved: Option<(usize, usize)> = None;
    // Each row springs from the slot it had to the one it has; the step
    // between slots is last frame's.
    let step_key = egui::Id::new("src-row-step");
    let step = ui.ctx().data(|d| d.get_temp::<f32>(step_key)).unwrap_or(64.0);
    let spring = Theme::get(ui.ctx()).motion.default_spatial;
    // The row whose arrow was pressed is lifted while it moves: a dragged
    // list item's surface and shadow over the row it passes, which slides
    // under it.
    let lifted_key = egui::Id::new("src-lifted");
    let lifted = ui.ctx().data(|d| d.get_temp::<v::SourceKey>(lifted_key));
    let mut lift_next: Option<v::SourceKey> = None;
    // The group's title stays put while the rows slide.
    if !shown.is_empty() {
        rows::titles(ui);
    }
    let top = ui.cursor().top();
    for (n, key) in shown.iter().enumerate() {
        let Some(at) = order.iter().position(|o| o.key == *key) else { continue };
        let view = state.downloads.sources.iter().find(|v| v.key == *key);
        let needs_setup = match key {
            v::SourceKey::Torrents => view.is_some_and(|v| !v.available),
            v::SourceKey::Soulseek => !state.downloads.soulseek.configured(),
            _ => false,
        };
        // Only what blocks a source, or went wrong with it.
        let note = match (key, view) {
            (v::SourceKey::Torrents, Some(v)) if !v.available => Some(Note::Blocker(st.need_prowlarr)),
            (v::SourceKey::Soulseek, _) if !state.downloads.soulseek.configured() => Some(Note::Blocker(st.need_account)),
            (v::SourceKey::Soulseek, Some(v)) if saved.source_on(*key) && !v.available => Some(Note::Error(st.not_connected)),
            (_, Some(v)) => v.status.as_deref().map(Note::Error),
            _ => None,
        };
        // A source that is not set up shows off (its saved choice stays).
        let mut on = order[at].on && !needs_setup;
        let slot = anim::f32(ui.ctx(), egui::Id::new(("src-slot", key.as_str())), n as f32, spring);
        let shift = TSTransform::from_translation(vec2(0.0, (slot - n as f32) * step));
        // Full while it travels, settling back down over its last quarter slot.
        let lift = if lifted == Some(*key) { ((slot - n as f32).abs() / 0.25).clamp(0.0, 1.0) } else { 0.0 };
        let draw = |ui: &mut Ui| {
            dimmed(ui, needs_setup, |ui| {
                row_note(ui, Id::DownloadSource(*key), note, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let up = ui.add(IconButton::new(Icon::ExpandLess).size(Size::Xs).enabled(n > 0).tooltip(st.up).id_salt(("src-up", key.as_str())));
                        if up.clicked() && n > 0 {
                            moved = order.iter().position(|o| o.key == shown[n - 1]).map(|to| (at, to));
                            lift_next = Some(*key);
                        }
                        let down = ui.add(
                            IconButton::new(Icon::ExpandMore)
                                .size(Size::Xs)
                                .enabled(n + 1 < shown.len())
                                .tooltip(st.down)
                                .id_salt(("src-down", key.as_str())),
                        );
                        if down.clicked() && n + 1 < shown.len() {
                            moved = order.iter().position(|o| o.key == shown[n + 1]).map(|to| (at, to));
                            lift_next = Some(*key);
                        }
                        ui.add_space(8.0);
                        let _ = ui.add(Switch::new(&mut on).enabled(!needs_setup).id_salt(("src-on", key.as_str())));
                    });
                })
            })
        };
        if lift > 0.0 {
            lifted_row(ui, shift, lift, draw);
        } else {
            ui.with_visual_transform(shift, draw);
        }
        if !needs_setup && on != order[at].on {
            order[at].on = on;
            p.download_sources = order.clone();
        }
    }
    if !shown.is_empty() {
        let h = (ui.cursor().top() - top) / shown.len() as f32;
        if h > 1.0 && (h - step).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(step_key, h));
        }
    }
    if let Some(k) = lift_next {
        ui.ctx().data_mut(|d| d.insert_temp(lifted_key, k));
    }
    if let Some((a, b)) = moved {
        order.swap(a, b);
        p.download_sources = order;
    }
}

/// A row drawn as a lifted surface (`lift` 0..1): on a layer just above the
/// page's, on a dragged list item's container with its shadow, moved by
/// `shift`. The page keeps its place for it as for any row.
fn lifted_row(ui: &mut Ui, shift: TSTransform, lift: f32, draw: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    let layer = egui::LayerId::new(ui.layer_id().order, egui::Id::new("src-lifted-layer"));
    ui.ctx().set_sublayer(ui.layer_id(), layer);
    ui.scope_builder(egui::UiBuilder::new().layer_id(layer), |ui| {
        ui.with_visual_transform(shift, |ui| {
            let (shadow, surface) = (ui.painter().add(egui::Shape::Noop), ui.painter().add(egui::Shape::Noop));
            let rect = ui.scope(draw).response.rect;
            // The whole row of the column, with the room the settings'
            // highlight gives a row.
            let r = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), rect.y_range()).expand2(vec2(12.0, 7.0));
            // It reaches past the column's edges as the highlight does: only
            // the scroll area's vertical clip applies (its shadow too).
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(egui::Rect::from_x_y_ranges(r.expand(24.0).x_range(), ui.clip_rect().y_range()));
            let mut sh = crate::Elevation::Level4.shadow();
            sh.color = sh.color.gamma_multiply(lift);
            painter.set(shadow, sh.as_shape(r, crate::tokens::shape::LARGE));
            let fill = s.surface_container_high.gamma_multiply(lift);
            let layer = s.on_surface.gamma_multiply(0.16 * lift);
            painter.set(
                surface,
                egui::Shape::Vec(vec![
                    egui::Shape::rect_filled(r, crate::tokens::shape::LARGE, fill),
                    egui::Shape::rect_filled(r, crate::tokens::shape::LARGE, layer),
                ]),
            );
        });
    });
}

/// «Soulseek»: the account (in the player), or — the owner's builds in
/// «Подробно» — `nikgob-slsk` on a server with its address and token; and
/// the sharing. Its switch is the row in «Откуда качать», its upload limit
/// in «Раздача».
fn soulseek(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    let saved = &state.downloads.soulseek;
    // The saved account fills the fields until something is typed (it may
    // come a moment after the page opens).
    if !pages.slsk_synced && saved.configured() {
        pages.slsk_remote = saved.remote;
        pages.slsk_user = saved.username.clone();
        pages.slsk_url = saved.remote_url.clone();
        pages.slsk_synced = true;
    }
    // Where it runs is the developer's choice; a saved server setup still
    // shows its fields.
    let dev = rows::level_shown(rows::mode(ui.ctx()), settings_help::Level::Dev);
    block(ui, Id::Soulseek, |ui| {
        if dev {
            let (pick, rects) = super::super::connected(ui, egui::Id::new("set-slsk-where"), &i18n::settings_tabs::slsk_where_in(i18n::locale()), usize::from(pages.slsk_remote), &[]);
            // The group fills its row: the ⓘ goes right after it, not beside
            // the fields' column further down.
            if let Some(last) = rects.last() {
                settings_help::help_button_beside(ui, Id::Soulseek, *last);
            }
            if let Some(n) = pick {
                pages.slsk_remote = n == 1;
                pages.slsk_synced = true;
            }
            ui.add_space(12.0);
        }
        let ready = if pages.slsk_remote {
            TextField::new(&mut pages.slsk_url, st.util_address)
                .placeholder("http://…:5031")
                .leading_icon(Icon::Link)
                .width(480.0)
                .id_salt("set-slsk-url")
                .show(ui);
            ui.add_space(8.0);
            let mut token = TextField::new(&mut pages.slsk_token, st.token_label).password(true);
            if saved.token_saved {
                token = token.placeholder(st.saved);
            }
            token.width(480.0).id_salt("set-slsk-token").show(ui);
            !pages.slsk_url.trim().is_empty()
        } else {
            TextField::new(&mut pages.slsk_user, st.nick).leading_icon(Icon::Person).width(480.0).id_salt("set-slsk-user").show(ui);
            ui.add_space(8.0);
            let mut pass = TextField::new(&mut pages.slsk_pass, st.password).password(true);
            if saved.password_saved {
                pass = pass.placeholder(st.saved);
            }
            pass.width(480.0).id_salt("set-slsk-pass").show(ui);
            !pages.slsk_user.trim().is_empty() && (saved.password_saved || !pages.slsk_pass.is_empty())
        };
        ui.add_space(8.0);
        if ui.add(Button::new(i18n::common().save).style(Style::Tonal).enabled(ready).id_salt("set-slsk-save")).clicked() {
            out.push(Command::SetSoulseek(v::SoulseekForm {
                remote: pages.slsk_remote,
                username: pages.slsk_user.trim().to_owned(),
                password: std::mem::take(&mut pages.slsk_pass),
                remote_url: pages.slsk_url.trim().to_owned(),
                remote_token: std::mem::take(&mut pages.slsk_token),
            }));
        }
    });
    // Without an account there is nothing to share: the row waits for it.
    let account = saved.configured();
    // Off while there is no account (the saved choice stays).
    let mut share = saved.share && account;
    dimmed(ui, !account, |ui| {
        row(ui, Id::SoulseekShare, |ui| {
            ui.add(Switch::new(&mut share).enabled(account).id_salt("set-slsk-share"));
        });
    });
    if account && share != saved.share {
        out.push(Command::SetSoulseekShare { share, upload_limit_kbps: saved.upload_limit_kbps });
    }
}

/// «Отдача Soulseek» (kept in settings.toml): written when the slider is
/// let go (or a limit typed); the field shows the value under the pointer
/// meanwhile, and the one let go until it is saved.
fn soulseek_upload(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let saved = &state.downloads.soulseek;
    let account = saved.configured();
    let held_key = egui::Id::new("set-slsk-ul-held");
    let held = ui.ctx().data(|d| d.get_temp::<u32>(held_key)).filter(|v| *v != saved.upload_limit_kbps);
    dimmed(ui, !account || !saved.share, |ui| {
        let (dragging, released) = rows::speed(ui, Id::SoulseekUpload, held.unwrap_or(saved.upload_limit_kbps), account);
        match released.or(dragging) {
            Some(v) => ui.ctx().data_mut(|d| {
                d.insert_temp(held_key, v);
            }),
            None if held.is_none() => ui.ctx().data_mut(|d| {
                d.remove::<u32>(held_key);
            }),
            None => {}
        }
        if let Some(v) = released
            && v != saved.upload_limit_kbps
        {
            out.push(Command::SetSoulseekShare { share: saved.share, upload_limit_kbps: v });
        }
    });
}

/// «Prowlarr»: its address and key, «Сохранить» and «Проверить»; the
/// check's answer in words (a failure in the error colour: it is one).
fn prowlarr(ui: &mut Ui, state: &ViewState, p: &Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let st = i18n::settings_tabs();
    if pages.indexer_url.is_empty() && !p.prowlarr_url.is_empty() {
        pages.indexer_url = p.prowlarr_url.clone();
    }
    block(ui, Id::Prowlarr, |ui| {
        TextField::new(&mut pages.indexer_url, st.prowlarr_address)
            .placeholder("https://…")
            .leading_icon(Icon::Link)
            .width(480.0)
            .id_salt("set-prowlarr-url")
            .show(ui);
        ui.add_space(8.0);
        let mut key = TextField::new(&mut pages.indexer_key, st.api_key_label).password(true);
        if p.prowlarr_configured {
            key = key.placeholder(st.saved);
        }
        key.width(480.0).id_salt("set-prowlarr-key").show(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(
                    Button::new(i18n::common().save)
                        .style(Style::Tonal)
                        .enabled(!pages.indexer_url.trim().is_empty() && !pages.indexer_key.trim().is_empty())
                        .id_salt("set-prowlarr-save"),
                )
                .clicked()
            {
                out.push(Command::SetIndexer {
                    url: pages.indexer_url.trim().to_owned(),
                    api_key: std::mem::take(&mut pages.indexer_key).trim().to_owned(),
                });
            }
            // Whether Prowlarr answers with what is typed, or else saved (no search).
            let checking = state.indexer_check.as_ref().is_some_and(|c| c.checking);
            let typed = !pages.indexer_url.trim().is_empty() && !pages.indexer_key.trim().is_empty();
            if (p.prowlarr_configured || typed)
                && ui.add(Button::new(st.check).style(Style::Text).enabled(!checking).id_salt("set-prowlarr-check")).clicked()
            {
                out.push(Command::CheckIndexer { url: pages.indexer_url.trim().to_owned(), api_key: pages.indexer_key.trim().to_owned() });
            }
        });
        if let Some(c) = &state.indexer_check {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if c.checking {
                    ui.add(LoadingIndicator::new().size(20.0));
                }
                let color = if c.ok || c.checking { s.on_surface_variant } else { s.error };
                ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(&c.text, color)).wrap());
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use egui::{Context, Rect, pos2, vec2};
    use nikgob_view::{Command, SoulseekView, ViewState};

    use super::super::super::PageState;
    use crate::{Theme, typography};

    fn context() -> Context {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        ctx
    }

    /// Every control of «Soulseek» and «Отдача Soulseek» that takes a
    /// click is clicked: without an account the sharing and its limit do
    /// nothing, with one they work.
    #[test]
    fn soulseek_sharing_waits_for_an_account() {
        for account in [false, true] {
            let ctx = context();
            let mut state = ViewState::default();
            state.downloads.soulseek = SoulseekView {
                username: if account { "me".into() } else { String::new() },
                password_saved: account,
                share: true,
                ..SoulseekView::default()
            };
            let mut pages = PageState::default();
            let mut frame = 0u32;
            let mut run = |events: Vec<egui::Event>, pages: &mut PageState| {
                let mut out = Vec::new();
                frame += 1;
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 900.0))),
                    time: Some(f64::from(frame) * 0.05),
                    events,
                    ..Default::default()
                };
                let mut done = ctx.run_ui(input, |ui| {
                    super::soulseek(ui, &state, pages, &mut out);
                    super::soulseek_upload(ui, &state, &mut out);
                });
                done.textures_delta.clear();
                out
            };
            run(Vec::new(), &mut pages);
            let clickable: Vec<Rect> = ctx.viewport(|v| {
                v.prev_pass
                    .widgets
                    .layers()
                    .flat_map(|(_, w)| w.iter())
                    .filter(|w| w.enabled && w.sense.senses_click())
                    .map(|w| w.interact_rect)
                    .collect()
            });
            assert!(!clickable.is_empty());
            let mut sent = Vec::new();
            for r in clickable {
                pages.slsk_remote = false;
                let at = r.center();
                let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
                sent.extend(run(vec![egui::Event::PointerMoved(at), button(true)], &mut pages));
                sent.extend(run(vec![button(false)], &mut pages));
                sent.extend(run(vec![egui::Event::PointerGone], &mut pages));
            }
            let sharing = sent.iter().filter(|c| matches!(c, Command::SetSoulseekShare { .. })).count();
            if account {
                assert!(sharing >= 2, "the switch and the limit: {sent:?}");
            } else {
                assert_eq!(sharing, 0, "{sent:?}");
            }
        }
    }
}
