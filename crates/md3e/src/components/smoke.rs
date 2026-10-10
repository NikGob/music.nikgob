//! Headless smoke tests: every component is laid out and painted over a few
//! frames (including opened overlays) to catch panics and id clashes.

use egui::{Context, Rect, pos2, vec2};

use super::*;
use crate::{Icon, Theme, typography};

fn context() -> Context {
    let ctx = Context::default();
    let mut fonts = egui::FontDefinitions::default();
    typography::install_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    crate::icons::install(&ctx);
    Theme::default().install(&ctx);
    ctx
}

fn frames(ctx: &Context, n: usize, mut f: impl FnMut(&mut egui::Ui, usize)) {
    for i in 0..n {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
            time: Some(i as f64 * 0.05),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| f(ui, i));
        out.textures_delta.clear();
    }
}

#[test]
fn inline_components_render() {
    let ctx = context();
    let mut on = false;
    let mut text = String::from("hello");
    frames(&ctx, 8, |ui, i| {
        ui.add(Switch::new(&mut on).with_icon(true));
        ui.add(FilterChip::new("All", i % 2 == 0));
        ui.add(AssistChip::new("FLAC").icon(Icon::Info));
        InputChip::new("tag").show(ui);
        ui.add(Divider::new().inset(16.0));
        ui.add(Fab::new(Icon::Add).label("Add"));
        ui.add(LoadingIndicator::new().contained(true));
        Card::filled().show(ui, |ui| ui.label("card"));
        MediaCard::new("Album")
            .subtitle("Artist")
            .badge("3/12")
            .show(ui);
        ListItem::new("Track")
            .supporting("Artist")
            .leading_number(1)
            .trailing_text("3:41")
            .drag_handle(true)
            .selected(i % 2 == 1)
            .trailing(|ui| {
                ui.label("x");
            })
            .show(ui);
        segmented_list(ui, 3, |n| {
            ListItem::new(format!("Item {n}")).selected(n == 1)
        });
        TextField::new(&mut text, "Label")
            .filled()
            .supporting("help")
            .leading_icon(Icon::Search)
            .show(ui);
        TextField::new(&mut text, "Label").error("bad").show(ui);
        SmallAppBar::new("Title")
            .leading(Icon::ArrowBack, "Back")
            .show(ui, |ui| {
                ui.label("t");
            });
        FlexibleAppBar::new("Album")
            .subtitle("2019")
            .show(ui, i as f32 * 10.0, |_| {});
        let mut q = String::new();
        SearchBar::new("bar").placeholder("Search").show(ui, &mut q);
        let r = ui.button("hover me");
        tooltip(r, "Tip", Some("Ctrl+T"));
    });
}

#[test]
fn overlays_render_open_and_closing() {
    let ctx = context();
    let mut dialog = true;
    let mut input_dialog = true;
    let mut side = true;
    let mut bottom = true;
    let mut text = String::new();
    let mut search_text = String::new();
    let mut menu_open = true;
    frames(&ctx, 12, |ui, i| {
        let ctx = ui.ctx().clone();
        if i == 8 {
            dialog = false;
            input_dialog = false;
            side = false;
            bottom = false;
            menu_open = false;
        }
        Dialog::new("d", "Delete?")
            .icon(Icon::Delete)
            .supporting("This cannot be undone.")
            .action("Cancel")
            .action("Delete")
            .show(&ctx, &mut dialog);
        TextInputDialog::new("t", "New playlist", "Name")
            .validate(|s| (s.len() > 20).then(|| "too long".to_owned()))
            .show(&ctx, &mut input_dialog, &mut text);
        SideSheet::new("s")
            .title("Queue")
            .show(&ctx, &mut side, |ui| {
                ui.label("content");
            });
        BottomSheet::new("b").show(&ctx, &mut bottom, |ui| {
            ui.label("content");
        });
        let anchor = Rect::from_min_size(pos2(100.0, 100.0), vec2(40.0, 40.0));
        let picked = Menu::new("m")
            .item(
                MenuItem::new("Play")
                    .icon(Icon::PlayArrow)
                    .shortcut("Space"),
            )
            .divider()
            .item(MenuItem::new("Delete").enabled(false))
            .item(MenuItem::new("Selected").selected(true))
            .popup(&ctx, anchor, &mut menu_open);
        assert!(picked.is_none());
        let fab_items = [
            FabMenuItem::new(Icon::UploadFile, "Files"),
            FabMenuItem::new(Icon::Folder, "Folder"),
        ];
        let mut fab = FabMenu::new("fab", Icon::Add, &fab_items).label("Add");
        if i == 3 {
            FabMenu::set_open(&ctx, "fab", true);
        }
        fab = fab.upwards(i % 2 == 0);
        let out = fab.show(ui);
        assert!(out.clicked.is_none());
        if i == 2 {
            Search::request_open(&ctx, "search");
        }
        Search::new("search")
            .placeholder("Search")
            .show(ui, &mut search_text, |ui| {
                ui.label("results");
            });
        let host = SnackbarHost::new("snack").show_close(true);
        let items = [
            SnackbarItem::new(1, "Added to favourites").action("Undo"),
            SnackbarItem::new(2, "Second"),
        ];
        let ev = host.show(&ctx, ctx.content_rect(), 96.0, &items);
        assert!(ev.action_clicked.is_none());
        SupportingPane::new().show(ui, |ui| {
            ui.label("pane");
        });
    });
}
