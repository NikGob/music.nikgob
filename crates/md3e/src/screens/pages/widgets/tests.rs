use super::initials;

/// The grid takes the same room whether its rows are laid out or not.
#[test]
fn grid_rows_out_of_sight_keep_their_room() {
    let height_with = |screen_h: f32| {
        let ctx = egui::Context::default();
        let mut h = 0.0;
        for i in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, screen_h))),
                time: Some(f64::from(i) * 0.05),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let items: Vec<u32> = (0..203).collect();
                let top = ui.cursor().top();
                let mut drawn = 0;
                super::grid(ui, &items, 160.0, |_, w| w + 56.0, |ui, _, w| {
                    drawn += 1;
                    ui.allocate_exact_size(egui::vec2(w, w + 56.0), egui::Sense::hover());
                    None
                });
                h = ui.cursor().top() - top;
                let _ = drawn;
            });
            out.textures_delta.clear();
        }
        h
    };
    assert_eq!(height_with(400.0), height_with(100_000.0));
}

#[test]
fn initials_skip_particles() {
    assert_eq!(initials("Boards of Canada"), "BC");
    assert_eq!(initials("Кино"), "К");
    assert_eq!(initials("Molchat Doma"), "MD");
    assert_eq!(initials("the xx"), "TX");
    assert_eq!(initials("Nick Cave & the Bad Seeds"), "NS");
    assert_eq!(initials(""), "");
}
