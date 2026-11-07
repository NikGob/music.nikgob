//! `NIKGOB_FRAMES=frames.csv`: one line per frame with how long it took, to
//! find what makes an animation stutter.
//!
//! Columns: seconds since start, the gap since the previous frame began,
//! the time the app's own code took (logic and drawing), what eframe did
//! after it (tessellation, texture uploads and painting: its CPU count for
//! the frame without the app's part and the frame limit's sleep; the vsync
//! wait is not in it), and a label (the screenshot step running, or
//! `live`), the font atlas's height, then for frames whose own code was slow, the scopes it went
//! to (`nikgob_md3e::prof`). When the run ends the median, 95th percentile and worst of
//! every label go to the log, with how many frames came over 20 ms after
//! the one before (a missed vsync at 60 Hz and up).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

/// Frames whose own code took longer get their scopes written down
/// (`NIKGOB_FRAMES_SLOW_MS` sets another bound, 0 for every frame).
const SLOW_MS: f32 = 12.0;

pub struct Frames {
    path: PathBuf,
    started: Instant,
    began: Option<Instant>,
    last_begin: Option<Instant>,
    rows: Vec<Row>,
    slow_ms: f32,
}

struct Row {
    at: f64,
    gap_ms: f32,
    app_ms: f32,
    slept_ms: f32,
    paint_ms: f32,
    label: String,
    /// The font atlas (width, height in texels).
    atlas: [usize; 2],
    /// The scopes of a slow frame (`nikgob_md3e::prof`).
    scopes: String,
}

impl Frames {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("NIKGOB_FRAMES")?;
        nikgob_md3e::prof::enable();
        Some(Self {
            path: path.into(),
            started: Instant::now(),
            began: None,
            last_begin: None,
            rows: Vec::new(),
            slow_ms: std::env::var("NIKGOB_FRAMES_SLOW_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(SLOW_MS),
        })
    }

    /// The frame's own code starts.
    pub fn begin(&mut self) {
        self.began = Some(Instant::now());
    }

    /// The frame's own code is done; `cpu` is eframe's count for the
    /// previous frame.
    pub fn end(&mut self, cpu: Option<f32>, label: &str, atlas: [usize; 2]) {
        let Some(began) = self.began.take() else { return };
        let now = Instant::now();
        let gap = self.last_begin.map_or(0.0, |l| began.duration_since(l).as_secs_f32() * 1000.0);
        self.last_begin = Some(began);
        // The previous frame's CPU time belongs to the previous row.
        if let (Some(cpu), Some(prev)) = (cpu, self.rows.last_mut()) {
            prev.paint_ms = (cpu * 1000.0 - prev.app_ms - prev.slept_ms).max(0.0);
        }
        let app_ms = now.duration_since(began).as_secs_f32() * 1000.0;
        let mut scopes = String::new();
        let mut spent = nikgob_md3e::prof::take();
        if app_ms >= self.slow_ms {
            spent.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (name, ms) in spent.iter().filter(|s| s.1 >= 0.5) {
                let _ = write!(scopes, "{name}={ms:.1} ");
            }
        }
        self.rows.push(Row {
            at: began.duration_since(self.started).as_secs_f64(),
            gap_ms: gap,
            app_ms,
            slept_ms: 0.0,
            paint_ms: f32::NAN,
            label: label.to_owned(),
            atlas,
            scopes,
        });
    }

    /// The frame limit slept this long after the frame's code.
    pub fn slept(&mut self, d: std::time::Duration) {
        if let Some(r) = self.rows.last_mut() {
            r.slept_ms = d.as_secs_f32() * 1000.0;
        }
    }

    fn write(&self) {
        let mut csv = String::from("t_s,gap_ms,app_ms,paint_ms,atlas_h,label,scopes\n");
        for r in &self.rows {
            let _ = writeln!(csv, "{:.4},{:.2},{:.2},{:.2},{},{},{}", r.at, r.gap_ms, r.app_ms, r.paint_ms, r.atlas[1], r.label.replace(',', " "), r.scopes.trim_end());
        }
        if let Err(e) = std::fs::write(&self.path, csv) {
            log::warn!("frames: cannot write {}: {e}", self.path.display());
        }
        if let Some(r) = self.rows.iter().max_by_key(|r| r.atlas[1]) {
            log::info!("frames: the font atlas grew to {}x{} texels", r.atlas[0], r.atlas[1]);
        }
        let mut labels: Vec<&str> = Vec::new();
        for r in &self.rows {
            if !labels.contains(&r.label.as_str()) {
                labels.push(&r.label);
            }
        }
        for label in labels {
            let pick = |f: fn(&Row) -> f32| {
                let mut v: Vec<f32> = self.rows.iter().filter(|r| r.label == label).map(f).filter(|x| x.is_finite()).collect();
                v.sort_by(f32::total_cmp);
                let at = |q: f32| v.get(((v.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
                (at(0.5), at(0.95), v.last().copied().unwrap_or(0.0))
            };
            let n = self.rows.iter().filter(|r| r.label == label).count();
            // Frames that missed their vsync (and the next one came late).
            let late = self.rows.iter().filter(|r| r.label == label && r.gap_ms > 20.0).count();
            let (g50, g95, gmax) = pick(|r| r.gap_ms);
            let (a50, a95, amax) = pick(|r| r.app_ms);
            let (p50, p95, pmax) = pick(|r| r.paint_ms);
            log::info!(
                "frames {label}: {n} frames, {late} over 20 ms, gap {g50:.1}/{g95:.1}/{gmax:.1} ms, app {a50:.1}/{a95:.1}/{amax:.1} ms, paint {p50:.1}/{p95:.1}/{pmax:.1} ms (median/p95/max)"
            );
        }
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        self.write();
    }
}
