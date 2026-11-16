//! `nikgob-phone`: the phone app on the PC, in a portrait window of a
//! phone's size (392×852 dp), over the real runtime, engine, API and mesh
//! (as a phone among the owner's devices).
//!
//! `nikgob-phone [--data <dir>] [--demo] [--name <name>] [--no-mesh]`;
//! `NIKGOB_DEMO=1` is `--demo` (the generated demo library in a throwaway
//! dir, the fake services; on the home network only with `NIKGOB_MESH=1`).
//! Keys for trying it: F2 lays a camera cut-out and the gesture bar over
//! the screen and gives the interface their insets; F3 brings up a
//! keyboard's worth of inset at the bottom; F4 switches the screen between
//! 360×800, 392×852 and 412×915; Esc and the mouse's back button are the
//! system's back. `NIKGOB_SHOT=out.png` with `NIKGOB_SHOT_STEPS` runs it
//! off the screen through steps and saves pictures (see shot.rs).

#![forbid(unsafe_code)]

#[allow(dead_code, clippy::all)]
#[path = "../../../../app/src/demo.rs"]
mod demo;
mod host;
mod shot;
#[allow(dead_code, clippy::all)]
#[path = "../../../../app/src/shot_fakes.rs"]
mod shot_fakes;

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, Id, LayerId, Order, Rect, Stroke, pos2, vec2};
use nikgob_mobile::{MobileEnv, MobileUi, SafeInsets};
use nikgob_view::Command;

/// Phone screens (dp): a narrow Android, the default, a large Pixel.
const PRESETS: [(f32, f32); 3] = [(360.0, 800.0), (392.0, 852.0), (412.0, 915.0)];
const DEFAULT_PRESET: usize = 1;
/// The status bar with a camera's cut-out, and the gesture area.
const STATUS_BAR: f32 = 28.0;
const GESTURE_BAR: f32 = 24.0;
/// A keyboard's height.
const KEYBOARD: f32 = 300.0;

fn usage(problem: &str) -> ! {
    eprintln!("nikgob-phone: {problem}\nusage: nikgob-phone [--data <dir>] [--demo] [--name <name>] [--no-mesh]");
    std::process::exit(2);
}

fn options() -> host::Options {
    let mut o = host::Options { demo: std::env::var_os("NIKGOB_DEMO").is_some(), ..host::Options::default() };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data" => o.data = Some(PathBuf::from(args.next().unwrap_or_else(|| usage("--data needs a folder")))),
            "--name" => o.name = Some(args.next().unwrap_or_else(|| usage("--name needs a name"))),
            "--demo" | "--demo-profile" => o.demo = true,
            "--no-mesh" => o.no_mesh = true,
            "--version" => {
                println!("nikgob-phone {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => usage(&format!("unknown argument {other}")),
        }
    }
    o
}

struct Phone {
    host: Arc<host::Host>,
    ui: MobileUi,
    commands: Vec<Command>,
    /// F2: the cut-out and the gesture bar; F3: the keyboard; a screenshot
    /// run's back.
    knobs: shot::Knobs,
    /// A screenshot run (`NIKGOB_SHOT`).
    shot: Option<shot::Shot>,
    preset: usize,
    /// The last clipboard request copied.
    copied: u64,
}

impl eframe::App for Phone {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The glyph atlas never as large as the GPU allows.
        raw_input.max_texture_side = Some(raw_input.max_texture_side.unwrap_or(4096).min(4096));
        if let Some(s) = self.shot.as_mut() {
            s.raw_input(raw_input);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let (f2, f3, f4, back) = ctx.input_mut(|i| {
            let back = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) || i.pointer.button_clicked(egui::PointerButton::Extra1);
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::F2),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F3),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F4),
                back,
            )
        });
        self.knobs.insets ^= f2;
        self.knobs.keyboard ^= f3;
        let back = back || std::mem::take(&mut self.knobs.back);
        if f4 {
            self.preset = (self.preset + 1) % PRESETS.len();
            let (w, h) = PRESETS[self.preset];
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
        }
        let (bass, bands) = self.host.levels();
        let env = MobileEnv {
            insets: SafeInsets {
                top: if self.knobs.insets { STATUS_BAR } else { 0.0 },
                bottom: if self.knobs.insets { GESTURE_BAR } else { 0.0 },
                ime_bottom: if self.knobs.keyboard { KEYBOARD } else { 0.0 },
            },
            back_pressed: back,
            bass,
            bands,
            reduced_motion: false,
        };
        {
            let state = self.host.runtime.state();
            // A screenshot run may lay states the demo cannot reach over it.
            let faked = self.shot.as_ref().and_then(|s| s.overlay(&state));
            let shown: &nikgob_view::ViewState = faked.as_ref().unwrap_or(&*state);
            self.ui.frame(ui, &env, shown, &mut self.commands);
            if let Some(c) = state.clipboard.as_ref().filter(|c| c.id != self.copied) {
                self.copied = c.id;
                ctx.copy_text(c.text.clone());
            }
            if let Some(s) = self.shot.as_mut() {
                s.frame(&ctx, &mut self.ui, shown, &mut self.commands, &mut self.knobs);
            }
        }
        self.host.runtime.send_all(self.commands.drain(..));
        let screen = ctx.content_rect();
        if self.knobs.insets {
            paint_insets(&ctx, screen);
        }
        if self.knobs.keyboard {
            paint_keyboard(&ctx, screen);
        }
    }
}

/// The camera's cut-out in the status bar and the gesture bar's handle.
fn paint_insets(ctx: &egui::Context, screen: Rect) {
    let p = ctx.layer_painter(LayerId::new(Order::Debug, Id::new("phone-insets")));
    p.rect_filled(Rect::from_min_size(screen.min, vec2(screen.width(), STATUS_BAR)), 0.0, Color32::from_black_alpha(90));
    p.circle_filled(pos2(screen.center().x, screen.top() + STATUS_BAR * 0.5), 6.0, Color32::BLACK);
    let handle = Rect::from_center_size(pos2(screen.center().x, screen.bottom() - GESTURE_BAR * 0.5), vec2(108.0, 4.0));
    p.rect_filled(handle, 2.0, Color32::from_white_alpha(200));
}

/// A keyboard's place at the bottom.
fn paint_keyboard(ctx: &egui::Context, screen: Rect) {
    let p = ctx.layer_painter(LayerId::new(Order::Debug, Id::new("phone-keyboard")));
    let kb = Rect::from_min_max(pos2(screen.left(), screen.bottom() - KEYBOARD), screen.max);
    p.rect_filled(kb, 0.0, Color32::from_gray(40));
    let keys = 10;
    let w = (kb.width() - 8.0) / keys as f32;
    for row in 0..4 {
        for k in 0..keys {
            let r = Rect::from_min_size(pos2(kb.left() + 4.0 + k as f32 * w, kb.top() + 12.0 + row as f32 * 54.0), vec2(w - 6.0, 44.0));
            p.rect(r, 6.0, Color32::from_gray(70), Stroke::NONE, egui::StrokeKind::Inside);
        }
    }
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let opts = options();
    let host = match host::Host::start(&opts) {
        Ok(h) => Arc::new(h),
        Err(e) => {
            eprintln!("nikgob-phone: {e}");
            std::process::exit(1);
        }
    };
    let [w, h] = shot::size().unwrap_or([PRESETS[DEFAULT_PRESET].0, PRESETS[DEFAULT_PRESET].1]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("music.nikgob")
        .with_app_id("music.nikgob.phone")
        .with_inner_size([w, h])
        .with_min_inner_size([320.0, 568.0])
        .with_max_inner_size([480.0, 1040.0]);
    // A screenshot run never shows up over the user's windows.
    if shot::offscreen() {
        viewport = viewport.with_position(shot::OFFSCREEN).with_active(false).with_taskbar(false);
    }
    let app_host = host.clone();
    let result = eframe::run_native(
        "nikgob-phone",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let mut fonts = egui::FontDefinitions::default();
            nikgob_md3e::typography::install_fonts(&mut fonts);
            ctx.set_fonts(fonts);
            nikgob_md3e::icons::install(&ctx);
            nikgob_md3e::Theme::new(nikgob_md3e::ColorScheme::baseline(true), true).install(&ctx);
            let _ = app_host.window.set(ctx);
            Ok(Box::new(Phone {
                host: app_host,
                ui: MobileUi::new(),
                commands: Vec::new(),
                knobs: shot::Knobs::default(),
                shot: shot::Shot::from_env(),
                preset: DEFAULT_PRESET,
                copied: 0,
            }))
        }),
    );
    host.shutdown();
    result
}
