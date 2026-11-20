//! What the player does with a new list (shuffled or in its order, by the
//! kind of list) and which of its optional buttons show.

/// The kind of a list that replaces the queue, for how it starts
/// ([`ListShuffles`]). Other lists (the library's tracks, a search, a
/// world) follow the shuffle button; radio, autoplay and the daylist have
/// their own setting ([`crate::Prefs::shuffle_radio`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ListKind {
    /// A mix of Главная («Микс дня», the named mixes, the tiles, a world's
    /// mixes, a blend) and an artist's «Слушать микс».
    Mix,
    /// A playlist made in the app, and «Любимые».
    OwnPlaylist,
    /// An album (of the library or of a service).
    Album,
    /// A playlist of a service: opened there, or imported or linked from it.
    ReadyPlaylist,
}

impl ListKind {
    pub const ALL: [Self; 4] = [Self::Mix, Self::OwnPlaylist, Self::Album, Self::ReadyPlaylist];
}

/// How a new list of one kind starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ListShuffle {
    /// «Вперемешку»: shuffle turns on.
    Shuffled,
    /// «По порядку»: shuffle turns off.
    InOrder,
    /// «По кнопке»: shuffle stays as the button is.
    #[default]
    Button,
}

impl ListShuffle {
    pub const ALL: [Self; 3] = [Self::Shuffled, Self::InOrder, Self::Button];

    /// The shuffle a new list plays with, from the shuffle `now`.
    pub fn apply(self, now: bool) -> bool {
        match self {
            Self::Shuffled => true,
            Self::InOrder => false,
            Self::Button => now,
        }
    }
}

/// [`ListShuffle`] of each [`ListKind`]: a mix is a mix and plays
/// shuffled; the user's own playlists play as the shuffle button is;
/// albums and ready playlists play in the order they were made in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ListShuffles {
    pub mixes: ListShuffle,
    pub own_playlists: ListShuffle,
    pub albums: ListShuffle,
    pub ready_playlists: ListShuffle,
}

impl Default for ListShuffles {
    fn default() -> Self {
        Self {
            mixes: ListShuffle::Shuffled,
            own_playlists: ListShuffle::Button,
            albums: ListShuffle::InOrder,
            ready_playlists: ListShuffle::InOrder,
        }
    }
}

impl ListShuffles {
    /// Every kind the same way.
    pub fn all(rule: ListShuffle) -> Self {
        Self { mixes: rule, own_playlists: rule, albums: rule, ready_playlists: rule }
    }

    pub fn of(&self, kind: ListKind) -> ListShuffle {
        match kind {
            ListKind::Mix => self.mixes,
            ListKind::OwnPlaylist => self.own_playlists,
            ListKind::Album => self.albums,
            ListKind::ReadyPlaylist => self.ready_playlists,
        }
    }

    pub fn set(&mut self, kind: ListKind, rule: ListShuffle) {
        match kind {
            ListKind::Mix => self.mixes = rule,
            ListKind::OwnPlaylist => self.own_playlists = rule,
            ListKind::Album => self.albums = rule,
            ListKind::ReadyPlaylist => self.ready_playlists = rule,
        }
    }

    /// The prefs from before the kinds had only «Перемешивание остаётся
    /// включённым»: on (`keep`), each kind takes its default; off, a new
    /// list always played in order, and every kind keeps doing so.
    pub fn from_keep(keep: bool) -> Self {
        if keep { Self::default() } else { Self::all(ListShuffle::InOrder) }
    }
}

/// An optional button of Now Playing and the player bar. Play, previous
/// and next always show; the keys work whatever is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum PlayerButton {
    /// «Скорость и эквалайзер» (Now Playing; the key E).
    Sound,
    Shuffle,
    Repeat,
    Like,
    /// «В плейлист…» (Now Playing).
    AddToPlaylist,
    /// The queue (Q).
    Queue,
    /// The lyrics (L).
    Lyrics,
}

impl PlayerButton {
    pub const ALL: [Self; 7] =
        [Self::Shuffle, Self::Repeat, Self::Like, Self::AddToPlaylist, Self::Queue, Self::Lyrics, Self::Sound];
}

/// Which [`PlayerButton`]s show: all but «Скорость и эквалайзер» (the
/// owner opens it with E) and «Очередь» (Q opens it, Now Playing has it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PlayerButtons {
    pub sound: bool,
    pub shuffle: bool,
    pub repeat: bool,
    pub like: bool,
    pub add_to_playlist: bool,
    pub queue: bool,
    pub lyrics: bool,
}

impl Default for PlayerButtons {
    fn default() -> Self {
        Self { sound: false, shuffle: true, repeat: true, like: true, add_to_playlist: true, queue: false, lyrics: true }
    }
}

impl PlayerButtons {
    pub fn shows(&self, b: PlayerButton) -> bool {
        match b {
            PlayerButton::Sound => self.sound,
            PlayerButton::Shuffle => self.shuffle,
            PlayerButton::Repeat => self.repeat,
            PlayerButton::Like => self.like,
            PlayerButton::AddToPlaylist => self.add_to_playlist,
            PlayerButton::Queue => self.queue,
            PlayerButton::Lyrics => self.lyrics,
        }
    }

    /// The switch of `b`.
    pub fn of_mut(&mut self, b: PlayerButton) -> &mut bool {
        match b {
            PlayerButton::Sound => &mut self.sound,
            PlayerButton::Shuffle => &mut self.shuffle,
            PlayerButton::Repeat => &mut self.repeat,
            PlayerButton::Like => &mut self.like,
            PlayerButton::AddToPlaylist => &mut self.add_to_playlist,
            PlayerButton::Queue => &mut self.queue,
            PlayerButton::Lyrics => &mut self.lyrics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_shuffle_own_playlists_follow_the_button_albums_and_ready_ones_keep_their_order() {
        let r = ListShuffles::default();
        assert_eq!(r.of(ListKind::Mix), ListShuffle::Shuffled);
        assert_eq!(r.of(ListKind::OwnPlaylist), ListShuffle::Button);
        assert_eq!(r.of(ListKind::Album), ListShuffle::InOrder);
        assert_eq!(r.of(ListKind::ReadyPlaylist), ListShuffle::InOrder);
        for now in [false, true] {
            assert!(ListShuffle::Shuffled.apply(now));
            assert!(!ListShuffle::InOrder.apply(now));
            assert_eq!(ListShuffle::Button.apply(now), now);
        }
    }

    #[test]
    fn each_kind_is_set_on_its_own() {
        let mut r = ListShuffles::default();
        for kind in ListKind::ALL {
            for rule in ListShuffle::ALL {
                r.set(kind, rule);
                assert_eq!(r.of(kind), rule);
            }
        }
        assert_eq!(r, ListShuffles::all(ListShuffle::Button));
    }

    #[test]
    fn the_old_switch_becomes_the_kinds() {
        assert_eq!(ListShuffles::from_keep(true), ListShuffles::default());
        for kind in ListKind::ALL {
            assert_eq!(ListShuffles::from_keep(false).of(kind), ListShuffle::InOrder);
        }
    }

    #[test]
    fn the_sound_and_queue_buttons_are_hidden_at_first() {
        let b = PlayerButtons::default();
        for button in PlayerButton::ALL {
            assert_eq!(b.shows(button), !matches!(button, PlayerButton::Sound | PlayerButton::Queue), "{button:?}");
        }
        let mut c = b;
        for button in PlayerButton::ALL {
            *c.of_mut(button) = !b.shows(button);
            assert_eq!(c.shows(button), !b.shows(button));
        }
    }
}
