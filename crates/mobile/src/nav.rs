//! The three destinations of the bottom bar, each with a stack of its own:
//! an album opened from Библиотека stays there while Главная is looked at,
//! and the bar's tab brings it back as it was. Pages of the whole app
//! (Главная, Библиотека on a tab, Настройки) switch the destination instead
//! of stacking. Поиск has no tab: the search bar at the top of Главная and
//! Библиотека opens it over the destination (Metrolist's).

use nikgob_core::i18n::{Locale, locale, nav_in};
use nikgob_view::{Command, LibraryTab, Route};

use nikgob_md3e::Icon;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tab {
    #[default]
    Home,
    Library,
    Settings,
}

impl Tab {
    pub const ALL: [Self; 3] = [Self::Home, Self::Library, Self::Settings];

    pub const fn index(self) -> usize {
        match self {
            Self::Home => 0,
            Self::Library => 1,
            Self::Settings => 2,
        }
    }

    pub const fn label_in(self, loc: Locale) -> &'static str {
        let n = nav_in(loc);
        match self {
            Self::Home => n.home,
            Self::Library => n.library,
            Self::Settings => n.settings,
        }
    }

    /// The destination's name in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(locale())
    }

    pub const fn icon(self) -> Icon {
        match self {
            Self::Home => Icon::Home,
            Self::Library => Icon::LibraryMusic,
            Self::Settings => Icon::Settings,
        }
    }
}

/// Where a route took the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// Another destination's first page.
    Tab(Tab),
    /// Библиотека on this chip.
    Library(LibraryTab),
    /// A page on the current destination's stack.
    Pushed,
    /// It is on screen already.
    Same,
}

/// How the page on screen came: +1 pushed, -1 back, 0 another destination.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub dir: f32,
    pub at: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Nav {
    tab: Tab,
    stacks: [Vec<Route>; 3],
    change: Option<Change>,
}

impl Nav {
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The page on top of the destination (`None`: its first page).
    pub fn top(&self) -> Option<&Route> {
        self.stacks[self.tab.index()].last()
    }

    pub fn depth(&self) -> usize {
        self.stacks[self.tab.index()].len()
    }

    pub fn change(&self) -> Option<Change> {
        self.change
    }

    fn changed(&mut self, dir: f32, now: f64) {
        self.change = Some(Change { dir, at: now });
    }

    /// Something over the page came or went (Поиск): the page on screen
    /// comes in as if pushed (+1) or gone back to (-1).
    pub fn moved(&mut self, dir: f32, now: f64) {
        self.changed(dir, now);
    }

    /// Opens `route` where it belongs.
    pub fn open(&mut self, route: Route, now: f64) -> Opened {
        match route {
            Route::Home => {
                let was = self.tab;
                self.tab = Tab::Home;
                self.stacks[Tab::Home.index()].clear();
                self.changed(if was == Tab::Home { -1.0 } else { 0.0 }, now);
                Opened::Tab(Tab::Home)
            }
            // From Настройки' own first page: the whole settings, as a page.
            Route::Settings if self.tab == Tab::Settings && self.top().is_none() => {
                self.stacks[Tab::Settings.index()].push(Route::Settings);
                self.changed(1.0, now);
                Opened::Pushed
            }
            Route::Settings => {
                self.tab = Tab::Settings;
                self.stacks[Tab::Settings.index()].clear();
                self.changed(0.0, now);
                Opened::Tab(Tab::Settings)
            }
            Route::Library(chip) => {
                let was = self.tab;
                self.tab = Tab::Library;
                self.stacks[Tab::Library.index()].clear();
                self.changed(if was == Tab::Library { -1.0 } else { 0.0 }, now);
                Opened::Library(chip)
            }
            r if self.top() == Some(&r) => Opened::Same,
            r => {
                let stack = &mut self.stacks[self.tab.index()];
                // Kept short: a long walk through artists and albums does
                // not pile up without end.
                if stack.len() >= 32 {
                    stack.remove(0);
                }
                stack.push(r);
                self.changed(1.0, now);
                Opened::Pushed
            }
        }
    }

    /// A tap on a destination of the bar. True when it was the one shown:
    /// its stack goes back to its first page.
    pub fn select(&mut self, tab: Tab, now: f64) -> bool {
        if tab == self.tab {
            let had = !self.stacks[tab.index()].is_empty();
            self.stacks[tab.index()].clear();
            if had {
                self.changed(-1.0, now);
            }
            return true;
        }
        self.tab = tab;
        self.changed(0.0, now);
        false
    }

    /// One page back on this destination; false at its first page.
    pub fn back(&mut self, now: f64) -> bool {
        if self.stacks[self.tab.index()].pop().is_some() {
            self.changed(-1.0, now);
            true
        } else {
            false
        }
    }
}

/// What the runtime must load for `route` to show.
pub fn load_for(route: &Route) -> Option<Command> {
    match route {
        Route::Home => Some(Command::LoadHome),
        Route::Library(_) => Some(Command::LoadLibrary),
        Route::Release(id) => Some(Command::LoadRelease(*id)),
        Route::Artist(id) => Some(Command::LoadArtist(*id)),
        Route::Playlist(id) => Some(Command::LoadPlaylist(*id)),
        Route::Stream(page) => Some(Command::LoadStream(page.clone())),
        Route::World(key) => Some(Command::LoadWorld(key.clone())),
        Route::Downloads(_) => Some(Command::LoadWishlist),
        Route::Mix { .. } | Route::NewReleases | Route::Settings | Route::Lyrics | Route::Marks | Route::Devices => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::{ArtistId, ReleaseId};

    fn release(n: u128) -> Route {
        Route::Release(ReleaseId(nikgob_core::model::Uuid::from_u128(n)))
    }

    #[test]
    fn each_destination_keeps_its_own_stack() {
        let mut nav = Nav::default();
        assert_eq!(nav.select(Tab::Library, 0.0), false);
        assert_eq!(nav.open(release(1), 0.1), Opened::Pushed);
        assert_eq!(nav.select(Tab::Settings, 0.2), false);
        assert_eq!(nav.top(), None);
        nav.select(Tab::Library, 0.3);
        assert_eq!(nav.top(), Some(&release(1)));
        assert_eq!(nav.change().map(|c| c.dir), Some(0.0));
    }

    #[test]
    fn the_shown_destination_tapped_again_goes_to_its_first_page() {
        let mut nav = Nav::default();
        nav.open(release(1), 0.0);
        nav.open(Route::Artist(ArtistId(nikgob_core::model::Uuid::from_u128(2))), 0.1);
        assert_eq!(nav.depth(), 2);
        assert!(nav.select(Tab::Home, 0.2));
        assert_eq!(nav.depth(), 0);
        assert_eq!(nav.change().map(|c| c.dir), Some(-1.0));
    }

    #[test]
    fn back_pops_and_stops_at_the_first_page() {
        let mut nav = Nav::default();
        nav.open(release(1), 0.0);
        assert_eq!(nav.open(release(1), 0.1), Opened::Same);
        assert!(nav.back(0.2));
        assert!(!nav.back(0.3));
    }

    #[test]
    fn app_wide_pages_switch_the_destination() {
        let mut nav = Nav::default();
        nav.open(release(1), 0.0);
        assert_eq!(nav.open(Route::Library(LibraryTab::Albums), 0.1), Opened::Library(LibraryTab::Albums));
        assert_eq!(nav.tab(), Tab::Library);
        assert_eq!(nav.open(Route::Settings, 0.2), Opened::Tab(Tab::Settings));
        assert_eq!(nav.open(Route::Settings, 0.25), Opened::Pushed, "the whole settings from Настройки");
        assert_eq!(nav.top(), Some(&Route::Settings));
        assert_eq!(nav.open(Route::Home, 0.3), Opened::Tab(Tab::Home));
        // Главная's own stack was emptied by the way back to it.
        assert_eq!(nav.top(), None);
    }

    #[test]
    fn what_each_page_loads() {
        assert_eq!(load_for(&Route::Home), Some(Command::LoadHome));
        assert_eq!(load_for(&Route::Settings), None);
        assert!(matches!(load_for(&release(3)), Some(Command::LoadRelease(_))));
    }
}
