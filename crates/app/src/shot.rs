//! `NIKGOB_SHOT=out.png [NIKGOB_SHOT_STEPS="route:library;wait:20;np;queue"]`:
//! drives the UI through a few steps, saves a screenshot and exits. Used to
//! look at screens without a human at the keyboard.
//!
//! Synthetic input (logical points): `move:x,y`, `click:x,y`, `rclick:x,y`,
//! `dblclick:x,y` (`click:x,y,frames` sets the wait after it), `xback:x,y` /
//! `xforward:x,y` (the mouse's side buttons), `drag:x1,y1,x2,y2`, `wheel:x,y,dy`,
//! `hwheel:x,y,dx` (a sideways wheel), `swheel:x,y,dy` (Shift + the wheel),
//! `key:space|escape|l|q|f11|f12` (`key:ctrl+2` and other egui key names too), `toasts` (a few snackbars).
//! `prefs:{json}` merges into the prefs.
//! `type:text,frames` types into the search bar one character every
//! `frames` frames (like a person; the bar's debounce applies).
//! `snap:path.png` saves a screenshot there and goes on (a sequence of
//! pictures in one run).
//! `album` opens the first album of the library (`album:Title` that one).
//! `first_run:step,picks` opens the first run at `sources`, `keep`,
//! `connect`, `lyrics` or `more` with the tiles `picks` picked
//! (`spotify+youtube+soundcloud+folder`, `local`).
//! `world:Title` chooses that world on the card of Главная,
//! `world_page:Title` opens its page (`world_page` the first world's).
//! `mix_page` opens the page of the first mix of Главная (`mix_page:Title`
//! that one, or the open world page's).
//! `lyrics_ui` opens where timing is made in the look
//! `NIKGOB_LYRICS_UI` picks (the control's menu, the dialog or the page);
//! `lyrics_menu` opens the control's own menu in any look.
//! `liked` opens «Любимые»; `play_playlist` plays the open playlist from
//! its start; `play_clip` plays the first clip of the open
//! playlist (`play_clip:song` its first stream song).
//! `find_song` opens «Найти песню…» for the first clip of the open
//! playlist (`find_song:Title` for the clip shown so, `find_song:song` «Другая
//! версия…» for its first stream song); `replace_first` replaces it with the
//! dialog's first result in the scope the dialog would offer.
//! `play_track:<id>` plays one track of the library by its id;
//! `play_search:n` the n-th track of the search's «Всё» answer;
//! `download` asks for the playing track's file.
//! `next` skips to the next track (`next:4` four times); `shuffle` turns
//! shuffle on or off (`shuffle:3` goes on 3 frames later, mid-change).
//! `wave` starts «Моя волна» (`wave:card` with the first card of «Миксы
//! для тебя» as its preset), `vibe` is «↻ Другой вайб».
//! `play_mix:daily|discover|mood|style|daylist|tile` plays a mix of Главная
//! (the first mood or style mix, the bento's daylist, its first tile;
//! `play_mix:tile:artist_of_day` the tile of that kind); `radio` starts «Радио по треку» from the first
//! library track (`radio:artist` the first artist's radio, `radio:album` the first
//! album's, `radio:playlist` the first playlist's).
//! `libfilter:text` types into the library's filter, `liblist:1` shows it
//! as a list, `sort:artist|title|year|added` sorts its albums.
//! `route:devices` and `devices_map:n` open «Карта сети» (`n` the device
//! whose passport opens: 0 this computer); `devices_add` opens «Добавить
//! устройство» (`devices_add:install` on «Установить») over Settings →
//! Межсеть or the map.
//! `reopen` closes the window and opens a new one, as the tray does.
//! `hide:ms` closes the window as its ✕ does (to the tray) and asks for it
//! back from the tray `ms` later (1000 by default).
//! `mark:name` names the frames that follow in `NIKGOB_FRAMES` (until a
//! run's first mark, each step names them after itself).
//! `NIKGOB_SHOT_SIZE=w,h` sets the window size; `NIKGOB_DATA=dir` runs on the
//! library in `dir` (as the demo does, with its fake services).
//! `NIKGOB_HOME_EPOCH=n` numbers the run for Главная's epoch (each start
//! draws another home; the same `n` and part of the day, the same one).
//! The window opens off the screen, unfocused and without a taskbar button,
//! so a run never jumps over the user's windows; `NIKGOB_SHOT_VISIBLE=1`
//! shows it.
//! `waitms:400` waits that long (not frames); `fake_clear` drops the
//! `fake:` states laid so far. The settings sweep (`--sweep`, see
//! [`crate::sweep`]) runs on this too: its `sweep_…` steps go to it.

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
    /// Where the screenshot asked by `snap:` goes (None: the final one).
    snap: Option<PathBuf>,
    /// The step the frames belong to, for `NIKGOB_FRAMES` (`mark:name`
    /// names them).
    label: String,
    marked: bool,
    /// `reopen` asked for a new window.
    reopen: bool,
    /// `hide` asked to go to the tray and come back after this long.
    hide: Option<std::time::Duration>,
    /// `waitms:` waits until then.
    until: Option<std::time::Instant>,
    /// A settings sweep: its steps, checks and files.
    sweep: Option<Box<crate::sweep::Run>>,
    /// The run ends without a last screenshot (a sweep saves its own).
    quiet_end: bool,
    /// A `.jpg` picture's quality.
    jpeg_quality: u8,
    /// The last frame's start, and whether a picture was saved in it (a
    /// slow frame then says nothing about the app).
    last_tick: Option<std::time::Instant>,
    saved: bool,
}

fn nums(arg: &str) -> Vec<f32> {
    arg.split(',').filter_map(|n| n.trim().parse().ok()).collect()
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

/// Where a screenshot run's window opens: far off every screen, so it
/// neither shows up over the user's windows nor takes the focus. (Not
/// −32000: Windows parks minimized windows there.)
pub const OFFSCREEN: [f32; 2] = [-16000.0, -16000.0];

/// A screenshot run keeps its window off the screen, unfocused and off the
/// taskbar (`NIKGOB_SHOT_VISIBLE=1` shows it where a normal start would).
pub fn offscreen() -> bool {
    (std::env::var_os("NIKGOB_SHOT").is_some() || crate::sweep::active()) && std::env::var_os("NIKGOB_SHOT_VISIBLE").is_none()
}

/// `NIKGOB_SHOT_SIZE=w,h`: the window size for a screenshot run.
pub fn window_size() -> Option<[f32; 2]> {
    if crate::sweep::active() {
        return crate::sweep::window_size();
    }
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
            snap: None,
            label: "start".to_owned(),
            marked: false,
            reopen: false,
            hide: None,
            until: None,
            sweep: None,
            quiet_end: false,
            jpeg_quality: 90,
            last_tick: None,
            saved: false,
        })
    }

    /// A settings sweep's run: its steps, driven by `run`, ending without a
    /// picture of its own.
    pub fn sweep(steps: Vec<String>, run: crate::sweep::Run, jpeg_quality: u8) -> Self {
        Self {
            path: PathBuf::new(),
            steps,
            wait: 40,
            requested: false,
            input: VecDeque::new(),
            fakes: Default::default(),
            snap: None,
            label: "sweep".to_owned(),
            marked: true,
            reopen: false,
            hide: None,
            until: None,
            sweep: Some(Box::new(run)),
            quiet_end: true,
            jpeg_quality,
            last_tick: None,
            saved: false,
        }
    }

    /// The view state with the `fake:` steps so far laid over it, if any.
    pub fn overlay(&self, state: &nikgob_view::ViewState) -> Option<nikgob_view::ViewState> {
        self.fakes.any().then(|| self.fakes.apply(state))
    }

    /// True once after `reopen`: the window closes and a new one opens,
    /// as when it is shown again from the tray.
    pub fn take_reopen(&mut self) -> bool {
        std::mem::take(&mut self.reopen)
    }

    /// Once after `hide`: how long the window stays in the tray.
    pub fn take_hide(&mut self) -> Option<std::time::Duration> {
        self.hide.take()
    }

    /// The step running, for the frame log.
    pub fn label(&self) -> &str {
        &self.label
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
        // A sweep times its frames (a frame that saved a picture is slow
        // on purpose and does not count).
        let now = std::time::Instant::now();
        if let (Some(run), Some(last)) = (self.sweep.as_mut(), self.last_tick) {
            run.tick(now - last, std::mem::take(&mut self.saved));
        }
        self.last_tick = Some(now);
        if let Some(img) = image {
            let [w, h] = img.size;
            let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let to = self.snap.take();
            match image::RgbaImage::from_raw(w as u32, h as u32, raw) {
                Some(buf) => {
                    let path = to.as_ref().unwrap_or(&self.path);
                    if let Err(e) = save(buf, path, self.jpeg_quality) {
                        log::warn!("shot: cannot save the screenshot to {}: {e}", path.display());
                    }
                }
                None => log::warn!("shot: the screenshot is not {w}×{h}"),
            }
            self.saved = true;
            if to.is_some() {
                self.requested = false;
                ctx.request_repaint();
                return false;
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return true;
        }
        ctx.request_repaint();
        if let Some(until) = self.until {
            if std::time::Instant::now() < until {
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
        if self.steps.is_empty() && self.quiet_end {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return true;
        }
        if self.steps.is_empty() {
            self.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
            return false;
        }
        let step = self.steps.remove(0);
        self.wait = 2;
        let (verb, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        // Once a step is `mark`ed, only marks rename the frames.
        if verb == "mark" {
            self.label = arg.to_owned();
            self.marked = true;
        } else if !self.marked && !matches!(verb, "wait" | "snap" | "typed") {
            self.label = step.clone();
        }
        match verb {
            "wait" => self.wait = arg.parse().unwrap_or(30),
            "waitms" => {
                self.wait = 0;
                self.until = Some(std::time::Instant::now() + std::time::Duration::from_millis(arg.parse().unwrap_or(400)));
            }
            "fake_clear" => {
                self.fakes.clear();
                self.wait = 1;
            }
            v if v.starts_with("sweep_") => {
                self.wait = match self.sweep.as_mut() {
                    Some(run) => run.step(verb, arg, ctx, ui, state, out, &mut self.steps, &mut self.fakes),
                    None => 0,
                };
            }
            "mark" => self.wait = 0,
            "reopen" => {
                self.reopen = true;
                self.wait = 40;
            }
            "hide" => {
                self.hide = Some(std::time::Duration::from_millis(arg.parse().unwrap_or(1000)));
                self.wait = 40;
            }
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
                    "albums" => Route::Library(LibraryTab::Albums),
                    "artists" => Route::Library(LibraryTab::Artists),
                    "trash" => Route::Library(LibraryTab::Trash),
                    "downloaded" => Route::Library(LibraryTab::Downloaded),
                    "downloads" => Route::Downloads(DownloadsTab::Active),
                    "wishlist" => Route::Downloads(DownloadsTab::Wishlist),
                    "confirm" => Route::Downloads(DownloadsTab::Confirm),
                    "history" => Route::Downloads(DownloadsTab::History),
                    "seeding" => Route::Downloads(DownloadsTab::Seeding),
                    "settings" => Route::Settings,
                    "new_releases" => Route::NewReleases,
                    "devices" => Route::Devices,
                    _ => Route::Home,
                };
                if let Route::Library(tab) = r {
                    ui.set_library_tab(tab);
                    if tab == LibraryTab::Downloaded {
                        out.push(Command::LoadDownloaded);
                    }
                }
                ui.navigate(ctx, r, out);
                self.wait = 30;
            }
            "first_run" => {
                // `first_run:connect,spotify+youtube+folder`: the first run
                // at that step with those tiles picked.
                let (step, picks) = arg.split_once(',').unwrap_or((arg, ""));
                if !ui.first_run_at(state, step, picks, out) {
                    log::warn!("shot: no first run step {step:?}");
                }
                self.wait = 40;
            }
            "world" | "world_page" => {
                // A world of the card by its title: chosen on the card, or
                // its page opened (`world_page:Title`).
                if let Some(w) = state.home.worlds.iter().find(|w| arg.is_empty() || w.title == arg) {
                    if verb == "world" {
                        ui.choose_world(ctx, &w.key);
                    } else {
                        ui.navigate(ctx, Route::World(w.key.clone()), out);
                    }
                }
                self.wait = 30;
            }
            "mix_page" => {
                // The page of the first mix of Главная (`mix_page:Title` the
                // one of that title, of the open world's page if it has it).
                let home = state.home.lanes.iter().find(|l| arg.is_empty() || l.title == arg);
                let world = state.world.as_ref().and_then(|v| v.mixes.iter().find(|l| l.title == arg).map(|l| (l, &v.world.key)));
                let route = match (home, world) {
                    (Some(l), _) => Some(Route::Mix { key: l.key, world: None }),
                    (None, Some((l, w))) => Some(Route::Mix { key: l.key, world: Some(w.clone()) }),
                    _ => None,
                };
                if let Some(r) = route {
                    ui.navigate(ctx, r, out);
                }
                self.wait = 30;
            }
            "album" => {
                // The first album of the loaded library (`album:Title` the
                // one of that title).
                let r = if arg.is_empty() {
                    state.library.releases.first()
                } else {
                    state.library.releases.iter().find(|r| r.title == arg)
                };
                if let Some(r) = r {
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
                // `like_album:n` the first n albums.
                for r in state.library.releases.iter().take(arg.parse().unwrap_or(1)) {
                    out.push(Command::Like(nikgob_view::LikeTarget::Release(r.id), !r.liked));
                }
                self.wait = 20;
            }
            "playlist" => {
                // The first user playlist.
                // `playlist:Name` picks one by name.
                if let Some(p) = state.library.playlists.iter().find(|p| !p.pinned && (arg.is_empty() || p.name == arg)) {
                    ui.navigate(ctx, Route::Playlist(p.id), out);
                }
                self.wait = 30;
            }
            "liked" => {
                // «Любимые».
                ui.navigate(ctx, Route::Playlist(nikgob_view::LIKED_PLAYLIST), out);
                self.wait = 30;
            }
            "play_playlist" => {
                // The open playlist from its start, as «Слушать» does.
                if let Some(p) = &state.playlist {
                    out.push(Command::PlayPlaylist { id: p.card.id, start: None, shuffle: false });
                }
                self.wait = 30;
            }
            "play_clip" => {
                // The first clip of the open playlist (`play_clip:song` plays
                // its first stream track that is not a clip).
                let want_clip = arg != "song";
                let pick = state.playlist.as_ref().and_then(|p| {
                    p.tracks.iter().find(|t| t.clip == want_clip && (want_clip || !t.has_local))
                });
                if let Some(t) = pick {
                    out.push(Command::PlayTracks { tracks: vec![t.id], start: 0, context: "Любимые".into() });
                }
                self.wait = 30;
            }
            "find_song" => {
                let pick = state.playlist.as_ref().and_then(|p| {
                    p.tracks.iter().find(|t| match arg {
                        "" => t.clip,
                        "song" => !t.clip && !t.has_local,
                        title => t.clip && t.title == title,
                    })
                });
                if let Some(t) = pick {
                    out.push(Command::FindSong { track: t.id, query: None });
                }
                self.wait = 40;
            }
            "replace_first" => {
                if let Some(f) = &state.find_song
                    && let Some(r) = f.results.first()
                {
                    out.push(Command::ReplaceTrack {
                        track: f.track,
                        with: r.remote.clone(),
                        youtube: state.prefs.replace_in_youtube && f.youtube_off.is_none(),
                    });
                }
                self.wait = 40;
            }
            "play_track" => {
                // `play_track:<id>` plays that one track of the library (a
                // stream one too) as a queue of its own.
                if let Ok(id) = arg.parse::<nikgob_core::model::TrackId>() {
                    out.push(Command::PlayTracks { tracks: vec![id], start: 0, context: "Скриншот".into() });
                }
                self.wait = 30;
            }
            "play" => {
                // `play:Title` plays the album of that title.
                let r = if arg.is_empty() {
                    state.library.releases.first()
                } else {
                    state.library.releases.iter().find(|r| r.title == arg)
                };
                if let Some(r) = r {
                    out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false });
                }
                self.wait = 20;
            }
            "next" => {
                for _ in 0..arg.parse().unwrap_or(1) {
                    out.push(Command::Next);
                }
                self.wait = 20;
            }
            "shuffle" => {
                out.push(Command::ToggleShuffle);
                self.wait = arg.parse().unwrap_or(30);
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
            "click" | "rclick" | "dblclick" | "xback" | "xforward" => {
                // `click:x,y,frames`: how long to wait after it (default 20;
                // 0 = the next step right after the release, for rapid clicks).
                let n = nums(arg);
                self.wait = 20;
                if let [x, y, ref rest @ ..] = n[..] {
                    let (b, times) = match verb {
                        "rclick" => (PointerButton::Secondary, 1),
                        "dblclick" => (PointerButton::Primary, 2),
                        "xback" => (PointerButton::Extra1, 1),
                        "xforward" => (PointerButton::Extra2, 1),
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
            // `hwheel:x,y,dx` turns a sideways wheel (`dx` lines; a part of
            // a line is a thumb wheel's small step), `swheel:x,y,dy` the
            // wheel with Shift held; `,frames` sets the wait after it.
            "hwheel" | "swheel" => {
                let n = nums(arg);
                if let [x, y, d, ..] = n[..] {
                    let shift = verb == "swheel";
                    self.input.push_back(vec![Event::PointerMoved(pos2(x, y))]);
                    self.input.push_back(vec![Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: if shift { egui::vec2(0.0, d) } else { egui::vec2(d, 0.0) },
                        phase: egui::TouchPhase::Move,
                        modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
                    }]);
                }
                self.wait = n.get(3).map_or(20, |f| *f as u32);
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
            // `volume:0` (a screenshot run with the real engine stays quiet).
            "volume" => {
                if let Ok(v) = arg.parse::<f32>() {
                    out.push(Command::SetVolume(v));
                }
                self.wait = 5;
            }
            "toasts" => {
                if let Some(t) = &state.player.current {
                    out.push(Command::Enqueue(vec![t.id]));
                }
                out.push(Command::AddMagnet("not a magnet".into()));
                out.push(Command::AddMagnet("magnet:?xt=urn:btih:0000000000000000000000000000000000000000".into()));
                self.wait = 30;
            }
            "queue" => {
                ui.open_queue(ctx, out);
                self.wait = 40;
            }
            "lyrics" => {
                ui.open_lyrics();
                self.wait = 40;
            }
            "lyrics_ui" => {
                // `lyrics_ui:4` shoots 4 frames in (mid-transition).
                nikgob_md3e::screens::lyrics_ui::open(ctx);
                self.wait = arg.parse().unwrap_or(60);
            }
            "lyrics_menu" => {
                nikgob_md3e::screens::lyrics_ui::open_menu(ctx);
                self.wait = arg.parse().unwrap_or(30);
            }
            "lyrics_edit" => {
                // The lyrics editor where the look has it, of the text shown
                // (a `fake:lyrics-state` one too).
                let shown = self.overlay(state);
                nikgob_md3e::screens::lyrics_ui::open_editor(ctx, shown.as_ref().unwrap_or(state));
                self.wait = arg.parse().unwrap_or(60);
            }
            "search" => {
                ui.set_search(arg, out);
                self.wait = 20;
            }
            "type" => {
                let (text, frames) = match arg.rsplit_once(',') {
                    Some((t, f)) if f.trim().parse::<u32>().is_ok() => (t, f.trim().parse().unwrap_or(6)),
                    _ => (arg, 6),
                };
                let chars: Vec<char> = text.chars().collect();
                for n in (1..=chars.len()).rev() {
                    let prefix: String = chars[..n].iter().collect();
                    self.steps.insert(0, format!("typed:{frames}:{prefix}"));
                }
                self.wait = 0;
            }
            "typed" => {
                if let Some((frames, text)) = arg.split_once(':') {
                    ui.set_search(text, out);
                    self.wait = frames.parse().unwrap_or(6);
                }
            }
            "snap" => {
                self.snap = Some(PathBuf::from(arg));
                self.requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
                self.wait = 0;
            }
            // `settings:sources` opens Settings on that tab / row
            // (`settings:lyrics-group` the lyrics group, as under the lyrics).
            "settings" => {
                ui.open_settings_at(arg);
                self.wait = 30;
            }
            // `devices_map` opens «Карта сети»; `devices_map:0` with this
            // computer's passport open, `devices_map:2` the second paired
            // device's.
            "devices_map" => {
                let shown = self.overlay(state);
                ui.open_devices_map(ctx, shown.as_ref().unwrap_or(state), arg.parse().ok(), out);
                self.wait = 40;
            }
            // `devices_add` opens «Добавить устройство» on «Подключить»
            // (`devices_add:install` on «Установить»); Settings → Межсеть
            // or «Карта сети» must be on screen (they draw it).
            "devices_add" => {
                let shown = self.overlay(state);
                ui.open_add_device(ctx, shown.as_ref().unwrap_or(state), arg == "install", out);
                self.wait = 40;
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
            // `play_search:2` plays the third track of the «Всё» answer as a
            // click on its row does (the top track first).
            "play_search" => {
                let a = &state.search.answer;
                let mut picks: Vec<nikgob_view::SearchPick> = Vec::new();
                if let Some(nikgob_view::TopResult::Track(t)) = &a.top {
                    picks.push(t.pick());
                }
                picks.extend(a.tracks.iter().map(|t| t.pick()));
                let start = arg.parse::<usize>().unwrap_or(0).min(picks.len().saturating_sub(1));
                if !picks.is_empty() {
                    out.push(Command::PlaySearch { tracks: picks, start, context: "поиска".into() });
                }
                self.wait = 60;
            }
            // `download` asks for the playing track's file («Скачать»).
            "download" => {
                if let Some(t) = &state.player.current {
                    out.push(Command::DownloadTracks(vec![t.id]));
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
            // `wave` starts «Моя волна» (`wave:card` as the first card of
            // «Миксы для тебя»), `vibe` turns it to another cluster.
            "wave" => {
                let preset = if arg == "card" { state.home.lanes.first().map(|l| l.key) } else { None };
                out.push(Command::PlayWave { preset });
                self.wait = 60;
            }
            "vibe" => {
                out.push(Command::OtherVibe);
                self.wait = 30;
            }
            "play_mix" => {
                let h = &state.home;
                let id = match arg {
                    "discover" => Some(nikgob_view::MixId::Discover),
                    "mood" => h.moods.first().map(|m| m.id),
                    "style" => h.style_mixes.first().map(|m| m.id),
                    "daylist" => Some(nikgob_view::MixId::Daylist),
                    "tile" => h.tiles.first().map(|m| m.id),
                    a if a.starts_with("tile:") => nikgob_view::TileKind::from_key(&a[5..]).map(nikgob_view::MixId::Tile),
                    _ => Some(nikgob_view::MixId::Daily),
                };
                if let Some(id) = id {
                    out.push(Command::PlayMix { id });
                }
                self.wait = 60;
            }
            "radio" => {
                let seed = match arg {
                    "artist" => state.library.artists.first().map(|a| nikgob_view::RadioSeed::Artist(a.id)),
                    "album" => state.library.releases.first().map(|r| nikgob_view::RadioSeed::Album(r.id)),
                    "playlist" => state.library.playlists.iter().find(|p| !p.pinned).map(|p| nikgob_view::RadioSeed::Playlist(p.id)),
                    _ => (state.library.tracks.first())
                        .or(state.home.mix.first())
                        .map(|t| nikgob_view::RadioSeed::Track(t.id)),
                };
                if let Some(seed) = seed {
                    out.push(Command::StartRadio { seed });
                }
                self.wait = 60;
            }
            // The library's filter, view and sort.
            "libfilter" => ui.set_library_filter(arg),
            "liblist" => Md3eUi::set_library_list(ctx, arg != "0"),
            "sort" => out.push(Command::SortReleases(match arg {
                "title" => nikgob_view::ReleaseSort::Title,
                "year" => nikgob_view::ReleaseSort::Year,
                "added" => nikgob_view::ReleaseSort::Added,
                _ => nikgob_view::ReleaseSort::Artist,
            })),
            "light" => out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs {
                theme: nikgob_view::ThemeMode::Light,
                ..state.prefs.clone()
            }))),
            _ => log::warn!("unknown shot step {step}"),
        }
        false
    }
}

/// Saves a picture: PNG, or JPEG (without alpha) at `quality` for `.jpg`.
fn save(buf: image::RgbaImage, path: &std::path::Path, quality: u8) -> image::ImageResult<()> {
    let jpeg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    if !jpeg {
        return buf.save(path);
    }
    let rgb = image::DynamicImage::ImageRgba8(buf).to_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality).encode_image(&rgb)
}
