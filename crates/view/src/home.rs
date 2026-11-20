//! Главная's view models: the shortcuts, the worlds and the sections.

use super::*;

/// One card of «Продолжить»: an album of the library, or an album of a
/// service that was streamed without joining the library (it opens the
/// service's page).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ContinueCard {
    Release(ReleaseCard),
    Stream(StreamAlbumCard),
}

/// A shortcut at the top of Главная: what the user is into now (an album,
/// a playlist or an artist), by plays that fade with a half-life of three
/// days. A click opens it, ▶ plays it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Shortcut {
    Release(ReleaseCard),
    /// The playlist and up to four covers of its albums (a 2×2 mosaic).
    Playlist(PlaylistCard, Vec<Cover>),
    Artist(ArtistCard),
}

/// An album of «Из твоей библиотеки» (or «С чего начать» without a
/// history): how many of its tracks were ever played.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShelfAlbum {
    pub card: ReleaseCard,
    pub heard: u32,
    pub total: u32,
}

/// A world of the card at the top of Главная («Миры»): a big body of
/// music to travel through and finish — a soundtrack franchise across all
/// its albums, or an artist's whole discography in the library — with how
/// far the user got.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct World {
    /// Stable across builds: the franchise's key, or `artist:` and its id.
    pub key: String,
    /// «MADE IN ABYSS», «Серега Пират».
    pub title: String,
    pub kind: WorldKind,
    /// The album on the stage and on its tile: the one playing, else the
    /// last played, else the first.
    pub release: Option<ReleaseId>,
    pub cover: Option<Cover>,
    /// A larger copy of the stage cover from a service when the album's own
    /// is small for the stage (it replaces it once loaded).
    pub cover_hd: Option<Cover>,
    /// «Слушаешь сейчас», «Слушал вчера», «Ещё не начинал».
    pub kicker: String,
    /// «Саундтрек · Kevin Penkin · 4 альбома · 134 трека» / «Исполнитель ·
    /// 7 релизов · 96 треков».
    pub facts: String,
    /// Tracks of it ever started, of all its tracks: «Ты слышал 36 из 134».
    pub heard: u32,
    pub total: u32,
    /// What its main button does.
    pub start: WorldStart,
    /// One of its tracks plays now.
    pub playing: bool,
    /// The tracks of the main button: «Дослушать» the unheard ones with a
    /// file in album order, «Начать» / «С начала» all from the first track
    /// of the first album.
    pub primary: Vec<TrackId>,
    /// «98 треков, 4 ч 35 мин: что ещё не слышал, по порядку альбомов».
    pub primary_tip: String,
    /// «Перемешать»: all its tracks, shuffled.
    pub shuffled: Vec<TrackId>,
    /// Its albums in the library, in order.
    pub albums: Vec<ReleaseId>,
    /// The three cards under the stage.
    pub cards: Vec<WorldCard>,
}

/// What a world is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorldKind {
    /// A game's or an anime's music across its albums.
    #[default]
    Soundtrack,
    /// An artist's discography.
    Artist,
}

impl WorldKind {
    /// «Саундтрек», «Исполнитель».
    pub const fn label_in(self, loc: Locale) -> &'static str {
        match self {
            Self::Soundtrack => view_in(loc).world_soundtrack,
            Self::Artist => view_in(loc).world_artist,
        }
    }

    /// The label in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(locale())
    }
}

/// The main button of a world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorldStart {
    /// «Дослушать»: something of it was heard, something is left.
    #[default]
    Finish,
    /// «Начать»: nothing of it was played.
    Begin,
    /// «С начала»: everything with a file was heard.
    Again,
}

impl WorldStart {
    pub const fn label_in(self, loc: Locale) -> &'static str {
        match self {
            Self::Finish => view_in(loc).world_finish,
            Self::Begin => view_in(loc).world_begin,
            Self::Again => view_in(loc).world_again,
        }
    }

    /// The label in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(locale())
    }
}

/// An album of a world's page with how much of it was heard.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldAlbum {
    pub card: ReleaseCard,
    pub heard: u32,
    pub total: u32,
}

/// A world's page ([`Route::World`], [`Command::LoadWorld`]).
#[derive(Clone, Debug, PartialEq)]
pub struct WorldView {
    pub world: World,
    /// Its albums in order.
    pub albums: Vec<WorldAlbum>,
    /// «Новое»: its new album of the services, not in the library.
    pub new_album: Option<StreamAlbumCard>,
    /// «Любимое»: its liked tracks (shown from three).
    pub liked: Vec<TrackRow>,
    /// «Миксы»: its mixes, none before the recommendations
    /// are built.
    pub mixes: Vec<LaneCard>,
}

/// One of the three cards under a world's stage: one click each.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorldCard {
    pub icon: WorldIcon,
    /// «Продолжить», «По порядку», «Новый альбом».
    pub kicker: String,
    /// «OST 1, трек 4».
    pub title: String,
    pub go: WorldGo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorldIcon {
    NowPlaying,
    Play,
    Next,
    Album,
    Favorite,
    Sound,
}

/// What a world's card does.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WorldGo {
    /// The player (what plays now).
    Player,
    /// Plays these tracks from the first.
    Play(Vec<TrackId>),
    /// Opens a library album.
    Release(ReleaseId),
    /// Opens an album of a service.
    Stream(RemoteId),
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HomeView {
    /// Главная is not built yet (the start of a run): the page shows the
    /// app's mark instead of an empty library.
    pub loading: bool,
    pub greeting_hour: u8,
    /// Something was ever played, liked or put in a playlist.
    pub history: bool,
    /// No song in the library and none ever played, as the library itself
    /// counts them (not what the rows below left out): Главная is the ways
    /// in, under «Музыки пока нет».
    #[serde(default)]
    pub library_empty: bool,
    /// The shortcuts at the top (six, eight on a wide window), or the
    /// recently added albums without a history.
    pub shortcuts: Vec<Shortcut>,
    /// The mixes, each row's by its key: «Миксы дня» ([`MixKey::Daily`], in
    /// their order) and «Миксы для тебя» — «Любимые» and the mix of the
    /// part of the day, the moments of the epoch in the order of now, then
    /// «На повторе».
    pub lanes: Vec<LaneCard>,
    /// The taste profiles «Смешать с…» offers.
    pub blend_profiles: Vec<BlendProfile>,
    /// «Слушать снова»: loved songs not played for a few days.
    pub listen_again: Vec<TrackRow>,
    /// «Из твоей библиотеки»: albums mostly unheard, other ones each day.
    pub from_library: Vec<ShelfAlbum>,
    /// The worlds of the card at the top, the one to open on first.
    pub worlds: Vec<World>,
    /// The library tracks of «Твой микс» (the hero's tracks; it plays with
    /// [`Command::PlayMix`] [`MixId::Daily`]).
    pub mix: Vec<TrackRow>,
    /// «Твой микс»: «30 треков · обновлён сегодня» and four covers. `None`
    /// until the recommendations are built the first time (or with an
    /// empty library).
    pub hero: Option<MixCard>,
    /// «Дейлист» ([`MixId::Daylist`]), the bento's big tile: title «Тягучий
    /// вечер четверга», subtitle «25 треков», four covers. `None` until the recommendations are built
    /// (or with an empty library).
    pub daylist: Option<MixCard>,
    /// The bento's smaller tiles of the day ([`MixId::Tile`]): «На повторе»
    /// first when it has enough songs, then a few other kinds that change
    /// every day (kicker «ИСПОЛНИТЕЛЬ ДНЯ», title «Kevin Penkin», subtitle
    /// «24 трека · лучшее»).
    pub tiles: Vec<MixCard>,
    /// «Под настроение»: the six moods, in the order for the part of the
    /// day (night: Спокойное, Мрачное first). Empty without any sound
    /// analysis.
    pub moods: Vec<MixCard>,
    /// «Твои миксы»: mixes by style, named by their artists («A, B и
    /// другие»), with a cover collage. Empty while too little is listened.
    pub style_mixes: Vec<MixCard>,
    /// «Новое для тебя»: up to 30 songs of the services new to the user
    /// (they play from the stream; «Слушать всё» is [`MixId::Discover`]).
    pub discover: Vec<StreamTrackRow>,
    /// «Новые релизы»: albums of the last 60 days by artists you listen to.
    pub new_releases: Vec<StreamAlbumCard>,
    /// Albums played lately, most recent first: library albums and streamed
    /// ones.
    pub continue_listening: Vec<ContinueCard>,
    /// «Миксы исполнителей»: a click plays the artist's radio
    /// ([`MixId::Artist`]).
    pub artist_mixes: Vec<ArtistCard>,
    pub long_unplayed: Vec<ReleaseCard>,
    pub recently_added: Vec<ReleaseCard>,
}

/// The card at the top of Главная («Миры») is in the app at all: off,
/// nobody sees it nor its row in Settings, whatever the prefs say, and
/// Главная starts with the greeting. Its code stays for its return.
pub const HOME_CARD: bool = false;
