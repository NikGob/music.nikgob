//! `NIKGOB_SHOT=out.png [NIKGOB_SHOT_STEPS="route:library;wait:20;np;queue"]`:
//! drives the UI through a few steps, saves a screenshot and exits. Used to
//! look at screens without a human at the keyboard.
//!
//! Synthetic input (logical points): `move:x,y`, `click:x,y`, `rclick:x,y`,
//! `dblclick:x,y` (`click:x,y,frames` sets the wait after it), `drag:x1,y1,x2,y2`, `wheel:x,y,dy`,
//! `key:space|escape|l|q|f11|f12` (`key:ctrl+2` and other egui key names too), `toasts` (a few snackbars).
//! `prefs:{json}` merges into the prefs.
//! `NIKGOB_SHOT_SIZE=w,h` sets the window size.

use std::collections::VecDeque;
use std::path::PathBuf;

use egui::{Context, Event, Modifiers, PointerButton, Pos2, UserData, ViewportCommand, pos2};
use nikgob_md3e::screens::Md3eUi;
use nikgob_view::{Command, DownloadsTab, LibraryTab, Route};

pub struct Shot {
    path: PathBuf,
    steps: Vec<String>,
    wait: u32,
    requested: bool,
    /// Raw input to feed, one entry per frame.
    input: VecDeque<Vec<Event>>,
    /// States laid over the view for the picture (`fake:…`).
    fakes: crate::shot_fakes::Fakes,
}

fn nums(arg: &str) -> Vec<f32> {
    arg.split(',').filter_map(|n| n.trim().parse().ok()).collect()
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

/// `NIKGOB_SHOT_SIZE=w,h`: the window size for a screenshot run.
pub fn window_size() -> Option<[f32; 2]> {
    std::env::var_os("NIKGOB_SHOT")?;
    match nums(&std::env::var("NIKGOB_SHOT_SIZE").ok()?)[..] {
        [w, h] => Some([w, h]),
        _ => None,
    }
}

impl Shot {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("NIKGOB_SHOT")?;
        let steps = std::env::var("NIKGOB_SHOT_STEPS")
            .unwrap_or_default()
            .split(';')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_owned())
            .collect();
        Some(Self {
            path: path.into(),
            steps,
            wait: 40,
            requested: false,
            input: VecDeque::new(),
            fakes: Default::default(),
        })
    }

    /// The view state with the `fake:` steps so far laid over it, if any.
    pub fn overlay(&self, state: &nikgob_view::ViewState) -> Option<nikgob_view::ViewState> {
        self.fakes.any().then(|| self.fakes.apply(state))
    }

    /// Feeds queued synthetic events into this frame's raw input.
    pub fn raw_input(&mut self, raw: &mut egui::RawInput) {
        if let Some(events) = self.input.pop_front() {
            raw.events.extend(events);
        }
    }

    fn press(&mut self, pos: Pos2, b: PointerButton, times: usize) {
        self.input.push_back(vec![Event::PointerMoved(pos)]);
        for _ in 0..times {
            self.input.push_back(vec![button(pos, b, true)]);
            self.input.push_back(vec![button(pos, b, false)]);
        }
    }

    /// Call once per frame, after drawing.
    pub fn frame(
        &mut self,
        ctx: &Context,
        ui: &mut Md3eUi,
        state: &nikgob_view::ViewState,
        out: &mut Vec<Command>,
    ) -> bool {
        // Saved?
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let [w, h] = img.size;
            let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, raw) {
                let _ = buf.save(&self.path);
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return true;
        }
        ctx.request_repaint();
        if self.wait > 0 || !self.input.is_empty() {
            self.wait = self.wait.saturating_sub(1);
            return false;
        }
        if self.requested {
            return false;
        }
        if self.steps.is_empty() {
            self.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
            return false;
        }
        let step = self.steps.remove(0);
        self.wait = 2;
        let (verb, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        match verb {
            "wait" => self.wait = arg.parse().unwrap_or(30),
            "size" => {
                if let Some((w, h)) = arg.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>())
                {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(w, h)));
                    self.wait = 20;
                }
            }
            "route" => {
                let r = match arg {
                    "library" => Route::Library(LibraryTab::All),
                    "tracks" => Route::Library(LibraryTab::Tracks),
                    "playlists" => Route::Library(LibraryTab::Playlists),
                    "trash" => Route::Library(LibraryTab::Trash),
                    "downloads" => Route::Downloads(DownloadsTab::Active),
                    "wishlist" => Route::Downloads(DownloadsTab::Wishlist),
                    "settings" => Route::Settings,
                    _ => Route::Home,
                };
                if let Route::Library(tab) = r {
                    ui.set_library_tab(tab);
                }
                ui.navigate(ctx, r, out);
                self.wait = 30;
            }
            "album" => {
                // The first album of the loaded library.
                if let Some(r) = state.library.releases.first() {
                    ui.navigate(ctx, Route::Release(r.id), out);
                }
                self.wait = 30;
            }
            "trash_album" => {
                // Deletes the first album of the library (into the Trash).
                if let Some(r) = state.library.releases.first() {
                    out.push(Command::TrashRelease(r.id));
                }
                out.push(Command::LoadTrash);
                self.wait = 30;
            }
            "like_album" => {
                if let Some(r) = state.library.releases.first() {
                    out.push(Command::Like(nikgob_view::LikeTarget::Release(r.id), !r.liked));
                }
                self.wait = 20;
            }
            "playlist" => {
                // The first user playlist.
                if let Some(p) = state.library.playlists.iter().find(|p| !p.pinned) {
                    ui.navigate(ctx, Route::Playlist(p.id), out);
                }
                self.wait = 30;
            }
            "play" => {
                if let Some(r) = state.library.releases.first() {
                    out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false });
                }
                self.wait = 20;
            }
            "seek" => {
                if let Ok(seconds) = arg.parse::<f32>()
                    && let Ok(position) = std::time::Duration::try_from_secs_f32(seconds)
                {
                    out.push(Command::Seek(position));
                }
                self.wait = 20;
            }
            "like" => {
                if let Some(t) = &state.player.current {
                    out.push(Command::Like(nikgob_view::LikeTarget::Track(t.id), !t.liked));
                }
                self.wait = 10;
            }
            "np" => {
                // `np:5` shoots 5 frames in (mid-transition).
                ui.set_now_playing(ctx, true);
                self.wait = arg.parse().unwrap_or(60);
            }
            "move" => {
                // `move:x,y,frames`: how long to wait after it (default 20;
                // 1 = the next step on the next frame, for fast sweeps).
                let n = nums(arg);
                self.wait = 20;
                if let [x, y, ref rest @ ..] = n[..] {
                    self.input.push_back(vec![Event::PointerMoved(pos2(x, y))]);
                    if let Some(&f) = rest.first() {
                        self.wait = f as u32;
                    }
                }
            }
            "click" | "rclick" | "dblclick" => {
                // `click:x,y,frames`: how long to wait after it (default 20;
                // 0 = the next step right after the release, for rapid clicks).
                let n = nums(arg);
                self.wait = 20;
                if let [x, y, ref rest @ ..] = n[..] {
                    let (b, times) = match verb {
                        "rclick" => (PointerButton::Secondary, 1),
                        "dblclick" => (PointerButton::Primary, 2),
                        _ => (PointerButton::Primary, 1),
                    };
                    self.press(pos2(x, y), b, times);
                    if let Some(&f) = rest.first() {
                        self.wait = f as u32;
                    }
                }
            }
            "drag" => {
                if let [x1, y1, x2, y2] = nums(arg)[..] {
                    let (a, b) = (pos2(x1, y1), pos2(x2, y2));
                    self.input.push_back(vec![Event::PointerMoved(a)]);
                    self.input.push_back(vec![button(a, PointerButton::Primary, true)]);
                    for i in 1..=12 {
                        self.input.push_back(vec![Event::PointerMoved(a.lerp(b, i as f32 / 12.0))]);
                    }
                    // Screenshot mid-drag with `hold` instead of releasing.
                    if !self.steps.first().is_some_and(|s| s == "hold") {
                        self.input.push_back(vec![button(b, PointerButton::Primary, false)]);
                    }
                }
                self.wait = 20;
            }
            "hold" => self.wait = 10,
            "wheel" => {
                if let [x, y, dy] = nums(arg)[..] {
                    self.input.push_back(vec![Event::PointerMoved(pos2(x, y))]);
                    self.input.push_back(vec![Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, dy),
                        phase: egui::TouchPhase::Move,
                        modifiers: Modifiers::NONE,
                    }]);
                }
                self.wait = 20;
            }
            "key" => {
                // `key:ctrl+2`: with Ctrl (Cmd on macOS).
                let (modifiers, arg) = match arg.strip_prefix("ctrl+") {
                    Some(rest) => (Modifiers::COMMAND, rest),
                    None => (Modifiers::NONE, arg),
                };
                let key = match arg {
                    "space" => Some(egui::Key::Space),
                    "escape" => Some(egui::Key::Escape),
                    "l" => Some(egui::Key::L),
                    "q" => Some(egui::Key::Q),
                    "f11" => Some(egui::Key::F11),
                    "f12" => Some(egui::Key::F12),
                    other => egui::Key::from_name(other),
                };
                if let Some(key) = key {
                    for pressed in [true, false] {
                        self.input.push_back(vec![Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers,
                        }]);
                    }
                }
                self.wait = 20;
            }
            // A few snackbars of different kinds, from real commands.
            "toasts" => {
                if let Some(t) = &state.player.current {
                    out.push(Command::Enqueue(vec![t.id]));
                }
                out.push(Command::AddMagnet("not a magnet".into()));
                out.push(Command::AddMagnet("magnet:?xt=urn:btih:0000000000000000000000000000000000000000".into()));
                self.wait = 30;
            }
            "queue" => {
                ui.open_queue();
                self.wait = 40;
            }
            "lyrics" => {
                ui.open_lyrics();
                self.wait = 40;
            }
            "search" => {
                ui.set_search(arg, out);
                self.wait = 20;
            }
            // `settings:sources` opens Settings on that tab / row.
            "settings" => {
                ui.open_settings_at(arg);
                self.wait = 30;
            }
            // `stream:album:spotify:sp-alb-gruppa` opens a page of a service.
            "stream" => {
                let mut it = arg.splitn(3, ':');
                if let (Some(kind), Some(service), Some(id)) = (it.next(), it.next(), it.next())
                    && let Some(service) = nikgob_view::Service::parse(service)
                {
                    let remote = nikgob_view::RemoteId::new(service, id);
                    let page = match kind {
                        "artist" => nikgob_view::StreamPage::Artist(remote),
                        "playlist" => nikgob_view::StreamPage::Playlist(remote),
                        _ => nikgob_view::StreamPage::Album(remote),
                    };
                    ui.navigate(ctx, Route::Stream(page), out);
                }
                self.wait = 40;
            }
            // `play_stream:spotify:sp-gruppa-2` plays one stream track.
            "play_stream" => {
                if let Some((service, id)) = arg.split_once(':')
                    && let Some(service) = nikgob_view::Service::parse(service)
                {
                    out.push(Command::PlayStreamTracks {
                        tracks: vec![nikgob_view::RemoteId::new(service, id)],
                        start: 0,
                        context: "Скриншот".into(),
                    });
                }
                self.wait = 30;
            }
            "artist" => {
                // The library artist named `arg`, or the first one.
                let a = state.library.artists.iter().find(|a| a.name == arg).or(state.library.artists.first());
                if let Some(a) = a {
                    ui.navigate(ctx, Route::Artist(a.id), out);
                }
                self.wait = 30;
            }
            // `fake:wishlist` etc. (see shot_fakes.rs).
            "fake" => {
                if !self.fakes.add(arg) {
                    log::warn!("unknown fake {arg}");
                }
                self.wait = 10;
            }
            "unfold" => {
                ui.unfold_sources();
                self.wait = 10;
            }
            // `prefs:{"ring_style":"Bars"}`: merges JSON into the prefs.
            "prefs" => {
                let mut v = serde_json::to_value(&state.prefs).unwrap_or_default();
                if let (Some(obj), Ok(serde_json::Value::Object(patch))) =
                    (v.as_object_mut(), serde_json::from_str::<serde_json::Value>(arg))
                {
                    obj.extend(patch);
                }
                if let Ok(p) = serde_json::from_value(v) {
                    out.push(Command::SetPrefs(Box::new(p)));
                }
                self.wait = 10;
            }
            "light" => out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs {
                theme: nikgob_view::ThemeMode::Light,
                ..state.prefs.clone()
            }))),
            _ => log::warn!("unknown shot step {step}"),
        }
        false
    }
}
