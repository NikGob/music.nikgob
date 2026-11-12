//! The first run (TODO 51, 57): whether it is due, and what its choices
//! write into the prefs. The page itself is the design system's.
//!
//! Its choices only ever set what the app already honours: the services'
//! switches, «Только скачанное», what a like downloads, whether the library
//! lists streams. Each is written from the prefs the first run started
//! from, so going back and choosing again undoes the earlier choice, and
//! what was not chosen keeps its value.

use crate::{Prefs, Service, UpgradeEvery};

/// The first run is shown at this start: it is still pending in the
/// prefs, and nothing says the player was used before — no music or
/// listening history in the library, no service signed in (a prefs file
/// lost on an old install must not throw its owner into it).
pub const fn due(pending: bool, has_music: bool, signed_in: bool) -> bool {
    pending && !has_music && !signed_in
}

/// Where the user takes music from: the first step's tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Spotify,
    YouTube,
    SoundCloud,
    /// Files on the disk: a folder is imported.
    Folder,
    /// «Только локальное»: no streaming service at all.
    LocalOnly,
}

impl Source {
    /// The tiles in their order.
    pub const ALL: [Self; 5] = [Self::Spotify, Self::YouTube, Self::SoundCloud, Self::Folder, Self::LocalOnly];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::YouTube => "YouTube Music",
            Self::SoundCloud => "SoundCloud",
            Self::Folder => "Папка на диске",
            Self::LocalOnly => "Только локальное",
        }
    }

    /// The streaming service the tile stands for.
    pub const fn service(self) -> Option<Service> {
        match self {
            Self::Spotify => Some(Service::Spotify),
            Self::YouTube => Some(Service::Youtube),
            Self::SoundCloud => Some(Service::Soundcloud),
            Self::Folder | Self::LocalOnly => None,
        }
    }
}

/// The tiles picked: several at once; «Только локальное» and the services
/// exclude each other (the folder goes with either).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picks(Vec<Source>);

impl Picks {
    pub fn has(&self, s: Source) -> bool {
        self.0.contains(&s)
    }

    /// Nothing is picked.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Picks `s`, or takes it back when it was picked.
    pub fn toggle(&mut self, s: Source) {
        if let Some(i) = self.0.iter().position(|x| *x == s) {
            self.0.remove(i);
            return;
        }
        if s == Source::LocalOnly {
            self.0.retain(|x| x.service().is_none());
        } else if s.service().is_some() {
            self.0.retain(|x| *x != Source::LocalOnly);
        }
        self.0.push(s);
        self.0.sort_by_key(|x| Source::ALL.iter().position(|a| a == x));
    }

    /// What was picked, in the tiles' order.
    pub fn iter(&self) -> impl Iterator<Item = Source> + '_ {
        self.0.iter().copied()
    }

    /// The services picked.
    pub fn services(&self) -> impl Iterator<Item = Service> + '_ {
        self.0.iter().filter_map(|s| s.service())
    }

    /// The step «Хранить музыку на компьютере?» is asked: a folder or
    /// «Только локальное» answers it already.
    pub fn asks_keep(&self) -> bool {
        !self.has(Source::Folder) && !self.has(Source::LocalOnly)
    }

    /// Something to connect: a service to sign in to, a folder to pick.
    pub fn connects(&self) -> bool {
        self.0.iter().any(|s| *s != Source::LocalOnly)
    }

    /// A service with an export of the listening history was picked
    /// (Spotify's data export, YouTube Music's Google Takeout).
    pub fn has_history(&self) -> bool {
        self.has(Source::Spotify) || self.has(Source::YouTube)
    }
}

/// «Хранить музыку на компьютере?»
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Keep {
    /// «Да»: likes download, the library is the files.
    Files,
    /// «Нет, только слушать»: nothing is downloaded by itself, the library
    /// lists the services' songs too (they play from the stream cache).
    Listen,
}

/// What the first run has been told so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub picks: Picks,
    /// The answer to the keep step; it counts only while that step is
    /// asked ([`Picks::asks_keep`]).
    pub keep: Option<Keep>,
}

impl Choices {
    /// The keep answer that counts.
    pub fn keep(&self) -> Option<Keep> {
        self.keep.filter(|_| self.picks.asks_keep())
    }
}

/// Writes the fields the first run's choices own into `p`, each from its
/// value in `start` (the prefs the first run began with) and `c`.
pub fn apply(p: &mut Prefs, start: &Prefs, c: &Choices) {
    let local = c.picks.has(Source::LocalOnly);
    let on = |was: bool, s: Source| !local && (was || c.picks.has(s));
    p.spotify_enabled = on(start.spotify_enabled, Source::Spotify);
    p.youtube_enabled = on(start.youtube_enabled, Source::YouTube);
    p.soundcloud_enabled = on(start.soundcloud_enabled, Source::SoundCloud);
    // Deezer is a catalogue without an account whose songs play from
    // YouTube Music: it goes with the other services.
    p.deezer_enabled = !local && start.deezer_enabled;
    let listen = c.keep() == Some(Keep::Listen);
    p.mixes_downloaded_only = local || (start.mixes_downloaded_only && !listen);
    match c.keep() {
        Some(Keep::Files) => {
            p.like_downloads = true;
            p.library_only_downloaded = true;
            p.lyrics_stream_keep = start.lyrics_stream_keep;
            p.upgrade_every = start.upgrade_every;
        }
        Some(Keep::Listen) => {
            p.like_downloads = false;
            p.library_only_downloaded = false;
            p.lyrics_stream_keep = 0;
            p.upgrade_every = UpgradeEvery::Never;
        }
        None => {
            p.like_downloads = start.like_downloads;
            p.library_only_downloaded = start.library_only_downloaded;
            p.lyrics_stream_keep = start.lyrics_stream_keep;
            p.upgrade_every = start.upgrade_every;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picks(of: &[Source]) -> Picks {
        let mut p = Picks::default();
        for s in of {
            p.toggle(*s);
        }
        p
    }

    #[test]
    fn it_is_due_only_for_a_player_never_used() {
        assert!(due(true, false, false), "a new install");
        assert!(!due(false, false, false), "an upgrade: its prefs file has no pending first run");
        assert!(!due(true, true, false), "a library with music");
        assert!(!due(true, false, true), "a service signed in");
    }

    #[test]
    fn local_only_and_the_services_exclude_each_other() {
        let mut p = picks(&[Source::Spotify, Source::Folder, Source::YouTube]);
        assert_eq!(p.iter().collect::<Vec<_>>(), [Source::Spotify, Source::YouTube, Source::Folder], "in the tiles' order");
        p.toggle(Source::LocalOnly);
        assert_eq!(p.iter().collect::<Vec<_>>(), [Source::Folder, Source::LocalOnly], "the folder stays");
        p.toggle(Source::SoundCloud);
        assert_eq!(p.iter().collect::<Vec<_>>(), [Source::SoundCloud, Source::Folder]);
        p.toggle(Source::SoundCloud);
        assert_eq!(p.iter().collect::<Vec<_>>(), [Source::Folder], "a second click takes it back");
    }

    #[test]
    fn what_the_picks_ask_for() {
        assert!(picks(&[Source::Spotify]).asks_keep());
        assert!(Picks::default().asks_keep(), "skipped: still asked");
        assert!(!picks(&[Source::YouTube, Source::Folder]).asks_keep(), "a folder keeps files");
        assert!(!picks(&[Source::LocalOnly]).asks_keep());
        assert!(!picks(&[Source::LocalOnly]).connects(), "nothing to sign in to");
        assert!(picks(&[Source::Folder]).connects());
        assert!(picks(&[Source::YouTube]).has_history() && !picks(&[Source::SoundCloud]).has_history());
    }

    #[test]
    fn a_service_picked_is_switched_on_and_the_rest_keep_theirs() {
        let start = Prefs::first_start();
        let mut p = start.clone();
        apply(&mut p, &start, &Choices { picks: picks(&[Source::Spotify]), keep: None });
        assert!(p.spotify_enabled, "off on a first start, on when picked");
        assert_eq!((p.youtube_enabled, p.soundcloud_enabled, p.deezer_enabled), (start.youtube_enabled, start.soundcloud_enabled, start.deezer_enabled));
        assert_eq!(p.mixes_downloaded_only, start.mixes_downloaded_only);
    }

    #[test]
    fn local_only_switches_the_services_off_and_plays_the_files() {
        let start = Prefs::first_start();
        let mut p = start.clone();
        apply(&mut p, &start, &Choices { picks: picks(&[Source::LocalOnly]), keep: Some(Keep::Listen) });
        assert!(!p.spotify_enabled && !p.youtube_enabled && !p.soundcloud_enabled && !p.deezer_enabled);
        assert!(p.mixes_downloaded_only, "«Только скачанное»");
        assert_eq!(p.like_downloads, start.like_downloads, "the keep answer does not count: the step is not asked");
    }

    #[test]
    fn listening_only_downloads_nothing_by_itself() {
        let start = Prefs { upgrade_every: UpgradeEvery::Week, lyrics_stream_keep: 1, mixes_downloaded_only: true, ..Prefs::first_start() };
        let mut p = start.clone();
        apply(&mut p, &start, &Choices { picks: picks(&[Source::YouTube]), keep: Some(Keep::Listen) });
        assert!(!p.like_downloads && !p.library_only_downloaded && !p.mixes_downloaded_only);
        assert_eq!((p.lyrics_stream_keep, p.upgrade_every), (0, UpgradeEvery::Never));
        apply(&mut p, &start, &Choices { picks: picks(&[Source::YouTube]), keep: Some(Keep::Files) });
        assert!(p.like_downloads && p.library_only_downloaded && p.mixes_downloaded_only);
        assert_eq!((p.lyrics_stream_keep, p.upgrade_every), (1, UpgradeEvery::Week));
    }

    #[test]
    fn choosing_again_undoes_the_choice_before() {
        let start = Prefs { like_downloads: false, ..Prefs::first_start() };
        let mut p = start.clone();
        apply(&mut p, &start, &Choices { picks: picks(&[Source::LocalOnly]), keep: None });
        apply(&mut p, &start, &Choices { picks: picks(&[Source::SoundCloud]), keep: Some(Keep::Files) });
        apply(&mut p, &start, &Choices::default());
        assert_eq!(p, start, "back at the start, nothing chosen: nothing changed");
    }
}
