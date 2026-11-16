//! Text laid out to a width: one line cut with «…», or a few centred.

use std::sync::Arc;
use std::time::Duration;

use egui::{Align, Color32, Galley, Pos2, Ui, text::TextWrapping};
use nikgob_md3e::TypeStyle;

/// `text` on one line at most `max_w` wide, cut with «…».
pub fn line(ui: &Ui, text: impl Into<String>, style: TypeStyle, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = style.job(text, color);
    job.wrap = TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// Paints `text` on one line with its top left at `at`; returns its width.
pub fn paint_line(ui: &Ui, at: Pos2, text: impl Into<String>, style: TypeStyle, color: Color32, max_w: f32) -> f32 {
    let g = line(ui, text, style, color, max_w);
    let w = g.size().x;
    ui.painter().galley(at, g, color);
    w
}

/// `text` in at most `rows` lines centred on x = 0 (paint it at the
/// centre's x), cut with «…».
pub fn centred(ui: &Ui, text: impl Into<String>, style: TypeStyle, color: Color32, max_w: f32, rows: usize) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.into(), style.text_format(color));
    job.halign = Align::Center;
    job.wrap = TextWrapping { max_width: max_w.max(1.0), max_rows: rows.max(1), break_anywhere: false, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// «3:42», «1:02:05».
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// «52 мин», «1 ч 12 мин».
pub fn total(d: Duration) -> String {
    let m = (d.as_secs() + 30) / 60;
    if m >= 60 { format!("{} ч {} мин", m / 60, m % 60) } else { format!("{m} мин") }
}

/// The Russian plural of `n`: `one` (1, 21…), `few` (2–4, 22…), `many`.
pub fn plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (n10, n100) = (n % 10, n % 100);
    if n10 == 1 && n100 != 11 {
        one
    } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
        few
    } else {
        many
    }
}

/// «14 треков».
pub fn tracks(n: u64) -> String {
    format!("{n} {}", plural(n, "трек", "трека", "треков"))
}

/// The first letters of up to two words, for an avatar without a photo.
pub fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_as_a_clock() {
        assert_eq!(duration(Duration::from_secs(222)), "3:42");
        assert_eq!(duration(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(total(Duration::from_secs(52 * 60 + 10)), "52 мин");
        assert_eq!(total(Duration::from_secs(72 * 60)), "1 ч 12 мин");
    }

    #[test]
    fn russian_plurals() {
        assert_eq!(tracks(1), "1 трек");
        assert_eq!(tracks(3), "3 трека");
        assert_eq!(tracks(11), "11 треков");
        assert_eq!(tracks(21), "21 трек");
        assert_eq!(tracks(14), "14 треков");
        assert_eq!(initials("Massive Attack"), "MA");
        assert_eq!(initials("земфира"), "З");
    }
}
