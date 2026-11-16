//! `NIKGOB_SHOT=out.png [NIKGOB_SHOT_STEPS="tab:library;chip:albums;snap:a.png;album"]
//! [NIKGOB_SHOT_SIZE=392,852]`: the phone drives itself through the steps
//! in a window off the screen, saves the pictures and exits, as the
//! desktop's screenshot runs do. The picture is md3e's (`shot`), the
//! `fake:` states are the desktop's (`shot_fakes`).
//!
//! The interface's own verbs are [`MobileUi::shot_step`]'s: `tab:home|
//! library|settings|search`, `search:text`, `chip:playlists|albums|artists|
//! tracks`, `np` (`np:n` n frames in), `close`, `lyrics`, `queue`,
//! `album[:Title]`, `artist[:Name]`, `playlist[:Name]`, `liked`, `menu`,
//! `output`. Here: `wait:n` (frames), `waitms:n`, `snap:path` (a picture,
//! and on), `size:360x800|392x852|412x915`, `play[:Title]` (an album of the
//! library from its start), `next`, `pause`, `like`, `volume:v`,
//! `click:x,y[,frames]`, `drag:x1,y1,x2,y2` (`hold` right after it keeps
//! the finger down for the picture), `key:escape|space|…`, `back`,
//! `insets[:off]` (the status bar and the gesture bar), `keyboard[:off]`,
//! `fake:<what>` (`fake_clear` drops them), `prefs:{json}`, `light`,
//! `dark`. The last picture goes to `NIKGOB_SHOT` when the steps run out.
//! `NIKGOB_SHOT_VISIBLE=1` shows the window.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Context, Event, Modifiers, PointerButton, Pos2, UserData, ViewportCommand, pos2};
use nikgob_mobile::MobileUi;
use nikgob_view::{Command, LikeTarget, ViewState};

/// Where the window opens on a run: far off every screen.
pub const OFFSCREEN: [f32; 2] = [-16000.0, -16000.0];

/// A run keeps its window off the screen (`NIKGOB_SHOT_VISIBLE=1` shows it).
pub fn offscreen() -> bool {
    std::env::var_os("NIKGOB_SHOT").is_some() && std::env::var_os("NIKGOB_SHOT_VISIBLE").is_none()
}

/// `NIKGOB_SHOT_SIZE=w,h` (or `wxh`): the screen of a run.
pub fn size() -> Option<[f32; 2]> {
    std::env::var_os("NIKGOB_SHOT")?;
    parse_size(&std::env::var("NIKGOB_SHOT_SIZE").ok()?)
}

/// «392,852» or «392x852».
pub fn parse_size(arg: &str) -> Option<[f32; 2]> {
    match nums(&arg.replace('x', ","))[..] {
        [w, h] if w > 0.0 && h > 0.0 => Some([w, h]),
        _ => None,
    }
}

fn nums(arg: &str) -> Vec<f32> {
    arg.split(',').filter_map(|n| n.trim().parse().ok()).collect()
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

/// What a step asks of the window around the interface.
#[derive(Default)]
pub struct Knobs {
    pub insets: bool,
    pub keyboard: bool,
    /// The system's back on the next frame.
    pub back: bool,
}

pub struct Shot {
    path: PathBuf,
    steps: VecDeque<String>,
    wait: u32,
    /// A picture was asked and has not come yet.
    requested: bool,
    /// Synthetic input, one entry per frame.
    input: VecDeque<Vec<Event>>,
    fakes: crate::shot_fakes::Fakes,
    /// Where the picture asked by `snap:` goes (None: the last one).
    snap: Option<PathBuf>,
    until: Option<Instant>,
}

impl Shot {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("NIKGOB_SHOT")?;
        let steps = std::env::var("NIKGOB_SHOT_STEPS")
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        Some(Self {
            path: path.into(),
            steps,
            // The library, Главная and the covers come in first.
            wait: 60,
            requested: false,
            input: VecDeque::new(),
            fakes: Default::default(),
            snap: None,
            until: None,
        })
    }

    /// The view state with the `fake:` states laid over it, if any.
    pub fn overlay(&self, state: &ViewState) -> Option<ViewState> {
        self.fakes.any().then(|| self.fakes.apply(state))
    }

    /// Feeds this frame's synthetic input.
    pub fn raw_input(&mut self, raw: &mut egui::RawInput) {
        if let Some(events) = self.input.pop_front() {
            raw.events.extend(events);
        }
    }

    /// Once a frame, after drawing. True when the run is over (the window
    /// closes).
    pub fn frame(&mut self, ctx: &Context, ui: &mut MobileUi, state: &ViewState, out: &mut Vec<Command>, knobs: &mut Knobs) -> bool {
        if let Some(img) = nikgob_md3e::shot::taken(ctx) {
            let to = self.snap.take();
            let path = to.as_ref().unwrap_or(&self.path);
            if let Err(e) = nikgob_md3e::shot::save(&img, path, 90) {
                log::warn!("shot: cannot save the picture to {}: {e}", path.display());
            }
            self.requested = false;
            if to.is_none() {
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return true;
            }
        }
        ctx.request_repaint();
        if let Some(until) = self.until {
            if Instant::now() < until {
                return false;
            }
            self.until = None;
        }
        if self.wait > 0 || !self.input.is_empty() {
            self.wait = self.wait.saturating_sub(1);
            return false;
        }
        if self.requested {
            return false;
        }
        let Some(step) = self.steps.pop_front() else {
            self.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
            return false;
        };
        let (verb, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        self.wait = 2;
        if let Some(w) = ui.shot_step(ctx, verb, arg, state, out) {
            self.wait = w;
            return false;
        }
        match verb {
            "wait" => self.wait = arg.parse().unwrap_or(30),
            "waitms" => {
                self.wait = 0;
                self.until = Some(Instant::now() + Duration::from_millis(arg.parse().unwrap_or(400)));
            }
            "snap" => {
                self.snap = Some(PathBuf::from(arg));
                self.requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
                self.wait = 0;
            }
            "size" => {
                if let Some([w, h]) = parse_size(arg) {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(w, h)));
                    self.wait = 20;
                }
            }
            "play" => {
                let r = state.library.releases.iter().find(|r| arg.is_empty() || r.title == arg);
                if let Some(r) = r {
                    out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false });
                }
                self.wait = 30;
            }
            "next" => {
                out.push(Command::Next);
                self.wait = 20;
            }
            "pause" => {
                out.push(Command::Pause);
                self.wait = 10;
            }
            "like" => {
                if let Some(t) = &state.player.current {
                    out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
                }
                self.wait = 10;
            }
            "volume" => {
                if let Ok(v) = arg.parse::<f32>() {
                    out.push(Command::SetVolume(v));
                }
                self.wait = 5;
            }
            "click" => {
                let n = nums(arg);
                self.wait = 20;
                if let [x, y, ref rest @ ..] = n[..] {
                    let p = pos2(x, y);
                    self.input.push_back(vec![Event::PointerMoved(p)]);
                    self.input.push_back(vec![button(p, true)]);
                    self.input.push_back(vec![button(p, false)]);
                    if let Some(&f) = rest.first() {
                        self.wait = f as u32;
                    }
                }
            }
            "drag" => {
                if let [x1, y1, x2, y2] = nums(arg)[..] {
                    let (a, b) = (pos2(x1, y1), pos2(x2, y2));
                    self.input.push_back(vec![Event::PointerMoved(a)]);
                    self.input.push_back(vec![button(a, true)]);
                    for i in 1..=12 {
                        self.input.push_back(vec![Event::PointerMoved(a.lerp(b, i as f32 / 12.0))]);
                    }
                    if self.steps.front().is_none_or(|s| s != "hold") {
                        self.input.push_back(vec![button(b, false)]);
                    }
                }
                self.wait = 20;
            }
            "hold" => self.wait = 10,
            "key" => {
                if let Some(key) = egui::Key::from_name(arg).or(match arg {
                    "escape" => Some(egui::Key::Escape),
                    "space" => Some(egui::Key::Space),
                    _ => None,
                }) {
                    for pressed in [true, false] {
                        self.input.push_back(vec![Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }]);
                    }
                }
                self.wait = 20;
            }
            "back" => {
                knobs.back = true;
                self.wait = 30;
            }
            "insets" => {
                knobs.insets = arg != "off";
                self.wait = 10;
            }
            "keyboard" => {
                knobs.keyboard = arg != "off";
                self.wait = 10;
            }
            "fake" => {
                if !self.fakes.add(arg) {
                    log::warn!("shot: unknown fake {arg}");
                }
                self.wait = 10;
            }
            "fake_clear" => {
                self.fakes.clear();
                self.wait = 2;
            }
            "prefs" => {
                let mut v = serde_json::to_value(&state.prefs).unwrap_or_default();
                if let (Some(obj), Ok(serde_json::Value::Object(patch))) = (v.as_object_mut(), serde_json::from_str::<serde_json::Value>(arg)) {
                    obj.extend(patch);
                }
                if let Ok(p) = serde_json::from_value(v) {
                    out.push(Command::SetPrefs(Box::new(p)));
                }
                self.wait = 10;
            }
            "light" | "dark" => {
                let theme = if verb == "light" { nikgob_view::ThemeMode::Light } else { nikgob_view::ThemeMode::Dark };
                out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs { theme, ..state.prefs.clone() })));
                self.wait = 30;
            }
            _ => log::warn!("shot: unknown step {step}"),
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_two_numbers_either_way() {
        assert_eq!(parse_size("392,852"), Some([392.0, 852.0]));
        assert_eq!(parse_size("360x800"), Some([360.0, 800.0]));
        assert_eq!(parse_size("360"), None);
        assert_eq!(parse_size("0x800"), None);
    }
}
