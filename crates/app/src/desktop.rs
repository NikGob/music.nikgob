//! The desktop around the player: tray item and media controls (Windows
//! media overlay / SMTC, media keys), both alive with or without a window.

use fastframe_now_playing::{self as np, NowPlaying};
use fastframe_tray::{self as tray, Tray};
use nikgob_core::i18n::{self, Locale};
use nikgob_view::{Command, Repeat, Status, ViewState};

/// What the desktop asked the app itself to do (beyond player commands).
#[derive(Default)]
pub struct Requests {
    pub show: bool,
    pub toggle: bool,
    pub quit: bool,
}

pub struct Desktop {
    tray: Option<Tray>,
    controls: NowPlaying,
    last_track: Option<String>,
    last_label: Option<bool>,
    last_locale: Locale,
}

pub use crate::icon_art::icon_rgba;

impl Desktop {
    pub fn start(wake: impl Fn() + Send + Sync + Clone + 'static, with_tray: bool) -> Self {
        let loc = i18n::locale();
        let n = i18n::nav_in(loc);
        let tray = with_tray.then(|| {
            Tray::spawn(
                tray::Config {
                    id: "music.nikgob",
                    title: "music.nikgob".into(),
                    icon: icon_rgba,
                    template_icon: None,
                    themed_icon: false,
                    menu_on_click: false,
                    menu: vec![
                        tray::MenuItem::action("show", n.tray_show_hide),
                        tray::MenuItem::Separator,
                        tray::MenuItem::action("play-pause", n.tray_play),
                        tray::MenuItem::action("next", n.tray_next),
                        tray::MenuItem::action("previous", n.tray_previous),
                        tray::MenuItem::Separator,
                        tray::MenuItem::action("quit", n.tray_quit),
                    ],
                },
                wake.clone(),
            )
        }).flatten();
        let mut app = np::App::new("nikgob", "music.nikgob");
        app.can_raise = true;
        app.can_quit = true;
        let controls = NowPlaying::start(app, wake);
        Self {
            tray,
            controls,
            last_track: None,
            last_label: None,
            last_locale: loc,
        }
    }

    /// The window may hide to the tray only if the tray is really there.
    pub fn can_hide(&self) -> bool {
        self.tray.as_ref().is_some_and(Tray::is_shown)
    }

    pub fn attach(&mut self) {
        if let Some(t) = self.tray.as_mut() {
            t.attach();
        }
    }

    /// Drains tray and media-control events into player commands and app
    /// requests, and publishes what is playing.
    pub fn poll(&mut self, state: &ViewState, out: &mut Vec<Command>) -> Requests {
        let mut req = Requests::default();
        if let Some(t) = &self.tray {
            for e in t.events() {
                match e {
                    tray::Event::Toggle | tray::Event::Menu("show") => req.toggle = true,
                    tray::Event::Show => req.show = true,
                    tray::Event::Menu("play-pause") => out.push(Command::TogglePlay),
                    tray::Event::Menu("next") => out.push(Command::Next),
                    tray::Event::Menu("previous") => out.push(Command::Previous),
                    tray::Event::Menu("quit") => req.quit = true,
                    tray::Event::Menu(_) => {}
                }
            }
        }
        for c in self.controls.commands() {
            match c {
                np::Command::Play => out.push(Command::Play),
                np::Command::Pause => out.push(Command::Pause),
                np::Command::PlayPause => out.push(Command::TogglePlay),
                np::Command::Stop => out.push(Command::Pause),
                np::Command::Next => out.push(Command::Next),
                np::Command::Previous => out.push(Command::Previous),
                np::Command::SeekBy(ms) => out.push(Command::SeekBy(ms)),
                np::Command::SetPosition { position, .. } => out.push(Command::Seek(position)),
                np::Command::SetVolume(v) => out.push(Command::SetVolume(v as f32)),
                np::Command::SetShuffle(on) => {
                    if on != state.player.shuffle {
                        out.push(Command::ToggleShuffle);
                    }
                }
                np::Command::SetRepeat(_) => out.push(Command::CycleRepeat),
                np::Command::Raise => req.show = true,
                np::Command::Quit => req.quit = true,
                np::Command::OpenUri(_) => {}
            }
        }
        self.publish(state);
        req
    }

    fn publish(&mut self, state: &ViewState) {
        let p = &state.player;
        let playback = match p.status {
            Status::Playing | Status::Buffering => np::Playback::Playing,
            Status::Paused => np::Playback::Paused,
            Status::Stopped => np::Playback::Stopped,
        };
        let track = p.current.as_ref().map(|t| np::Track {
            id: t.id.to_string(),
            title: t.title.clone(),
            artists: vec![t.artist.clone()],
            album: t.album.clone().unwrap_or_default(),
            duration: t.duration,
            art_file: t.cover.as_ref().and_then(|c| c.file()).map(std::path::Path::to_path_buf),
            ..np::Track::default()
        });
        let id = track.as_ref().map(|t| t.id.clone());
        let changed = id != self.last_track;
        self.last_track = id;
        self.controls.update(np::State {
            playback,
            track,
            position: p.position_now(),
            volume: Some(f64::from(p.volume)),
            shuffle: Some(p.shuffle),
            repeat: Some(match p.repeat {
                Repeat::Off => np::Repeat::Off,
                Repeat::One => np::Repeat::Track,
                Repeat::All => np::Repeat::Playlist,
            }),
            controls: np::Controls {
                play: true,
                pause: true,
                next: !state.queue.next.is_empty() || p.repeat != Repeat::Off,
                previous: p.current.is_some(),
                seek: p.current.is_some(),
            },
        });
        if changed {
            self.controls.seeked(p.position_now());
        }
        let playing = p.status == Status::Playing;
        let loc = i18n::locale();
        let locale_changed = self.last_locale != loc;
        if (locale_changed || self.last_label != Some(playing) || changed)
            && let Some(t) = self.tray.as_mut()
        {
            let n = i18n::nav_in(loc);
            if locale_changed {
                t.set_label("show", n.tray_show_hide);
                t.set_label("next", n.tray_next);
                t.set_label("previous", n.tray_previous);
                t.set_label("quit", n.tray_quit);
                self.last_locale = loc;
            }
            t.set_label("play-pause", if playing { n.tray_pause } else { n.tray_play });
            let tip = match &p.current {
                Some(c) => format!("music.nikgob\n{} — {}", c.title, c.artist),
                None => "music.nikgob".to_owned(),
            };
            t.set_tooltip(&tip);
            self.last_label = Some(playing);
        }    }
}
