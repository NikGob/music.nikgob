//! Where the window was and how big: kept in `window.json` next to the
//! prefs, so a new run (and the window made again after the tray) opens the
//! way it was left. Fullscreen and minimized states are never remembered;
//! a maximized window keeps the size it restores to.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const DEFAULT_SIZE: [f32; 2] = [1280.0, 800.0];
pub const MIN_SIZE: [f32; 2] = [360.0, 560.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// Inner size, logical points.
    pub size: [f32; 2],
    /// Outer top-left, logical points.
    pub pos: Option<[f32; 2]>,
    pub maximized: bool,
}

impl Geometry {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        let pair = |key: &str| -> Option<[f32; 2]> {
            let a = v.get(key)?.as_array()?;
            let x = a.first()?.as_f64()? as f32;
            let y = a.get(1)?.as_f64()? as f32;
            (x.is_finite() && y.is_finite()).then_some([x, y])
        };
        let size = pair("size")?;
        Some(Self {
            size: [size[0].max(MIN_SIZE[0]), size[1].max(MIN_SIZE[1])],
            pos: pair("pos"),
            maximized: v.get("maximized").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    fn to_json(self) -> Vec<u8> {
        let v = json!({ "size": self.size, "pos": self.pos, "maximized": self.maximized });
        serde_json::to_vec_pretty(&v).unwrap_or_default()
    }
}

/// `rect` in egui's points at `zoom` as the system's logical points.
fn unzoom(rect: egui::Rect, zoom: f32) -> egui::Rect {
    let k = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    egui::Rect::from_min_max((rect.min.to_vec2() * k).to_pos2(), (rect.max.to_vec2() * k).to_pos2())
}

/// Watches the window every frame and writes changes once they settle.
pub struct Keeper {
    /// None: nothing is remembered (screenshot runs).
    path: Option<PathBuf>,
    saved: Option<Geometry>,
    current: Option<Geometry>,
    changed: Option<Instant>,
}

impl Keeper {
    pub fn load(path: Option<PathBuf>) -> Self {
        let saved = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| Geometry::parse(&b));
        Self { path, saved, current: saved, changed: None }
    }

    pub fn apply(&self, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        if let Some(size) = crate::shot::window_size() {
            return builder.with_inner_size(size);
        }
        let Some(g) = self.current else {
            return builder.with_inner_size(DEFAULT_SIZE);
        };
        let mut builder = builder.with_inner_size(g.size).with_maximized(g.maximized);
        if let Some(pos) = g.pos {
            // A monitor that is gone is handled by `recover_offscreen`.
            builder = builder.with_position(pos);
        }
        builder
    }

    pub fn observe(&mut self, ctx: &egui::Context) {
        if self.path.is_none() {
            return;
        }
        // egui gives the window in its own points, which «Масштаб
        // интерфейса» (the zoom) makes larger; kept and restored in the
        // system's logical points, as the window is made before the zoom.
        let zoom = ctx.zoom_factor();
        let (inner, outer, minimized, maximized, fullscreen) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect.map(|r| unzoom(r, zoom)), v.outer_rect.map(|r| unzoom(r, zoom)), v.minimized, v.maximized, v.fullscreen)
        });
        if minimized == Some(true) || fullscreen == Some(true) {
            return;
        }
        let maximized = maximized == Some(true);
        let next = if maximized {
            // Keep the size it restores to.
            let base = self.current.unwrap_or(Geometry { size: DEFAULT_SIZE, pos: None, maximized: false });
            Geometry { maximized: true, ..base }
        } else {
            let Some(inner) = inner.filter(|r| r.width() >= 1.0 && r.height() >= 1.0) else {
                return;
            };
            Geometry {
                size: [inner.width().round(), inner.height().round()],
                pos: outer.map(|r| [r.min.x.round(), r.min.y.round()]),
                maximized: false,
            }
        };
        if self.current != Some(next) {
            self.current = Some(next);
            self.changed = Some(Instant::now());
        }
        // Dragging and resizing change it every frame: write when it rests.
        if self.changed.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
            self.flush();
        } else if self.changed.is_some() {
            ctx.request_repaint_after(Duration::from_millis(1100));
        }
    }

    pub fn flush(&mut self) {
        self.changed = None;
        let (Some(path), Some(g)) = (&self.path, self.current) else {
            return;
        };
        if self.saved == Some(g) {
            return;
        }
        if let Err(e) = std::fs::write(path, g.to_json()) {
            log::warn!("cannot save window geometry to {}: {e}", path.display());
            return;
        }
        self.saved = Some(g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_clamps_to_the_minimum() {
        let g = Geometry { size: [1100.0, 950.0], pos: Some([-1800.0, 40.0]), maximized: true };
        assert_eq!(Geometry::parse(&g.to_json()), Some(g));
        let small = Geometry::parse(br#"{"size":[100,100],"pos":null}"#).unwrap();
        assert_eq!(small, Geometry { size: MIN_SIZE, pos: None, maximized: false });
        assert_eq!(Geometry::parse(b"garbage"), None);
        assert_eq!(Geometry::parse(br#"{"pos":[1,2]}"#), None);
    }

    /// A window seen at a zoom of 1.5 (its points a third fewer) is kept at
    /// the size and place the system gives it.
    #[test]
    fn the_zoom_is_taken_out_of_the_kept_size() {
        let seen = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(800.0, 500.0));
        assert_eq!(unzoom(seen, 1.5), egui::Rect::from_min_size(egui::pos2(150.0, 60.0), egui::vec2(1200.0, 750.0)));
        assert_eq!(unzoom(seen, 1.0), seen);
        assert_eq!(unzoom(seen, f32::NAN), seen);
    }
}
