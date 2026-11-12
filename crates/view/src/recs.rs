//! The recommendations part of the view model (stage 4, docs/stage4.md §6):
//! the mix cards of the home page, what plays a mix or a radio, the
//! «Не рекомендовать» feedback and the radio line of the queue.
//!
//! The engine itself (`nikgob-recs`) stays behind the runtime: the view only
//! names things ([`Mood`], [`MixId`]) and shows what the runtime filled in.

use crate::{ArtistId, Cover, RemoteId, TrackId};

/// The six moods of «Под настроение» (the engine's own enum stays in the
/// runtime; this is its name in the view).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Mood {
    /// «Спокойное».
    Calm,
    /// «Энергичное».
    Energetic,
    /// «Мрачное».
    Dark,
    /// «Светлое».
    Bright,
    /// «Для фокуса».
    Focus,
    /// «Драйв».
    Drive,
}

impl Mood {
    pub const ALL: [Self; 6] = [Self::Calm, Self::Energetic, Self::Dark, Self::Bright, Self::Focus, Self::Drive];

    /// The card's title.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Calm => "Спокойное",
            Self::Energetic => "Энергичное",
            Self::Dark => "Мрачное",
            Self::Bright => "Светлое",
            Self::Focus => "Для фокуса",
            Self::Drive => "Драйв",
        }
    }
}

/// Which mix a card plays ([`crate::Command::PlayMix`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MixId {
    /// «Твой микс»: the hero, the same all day (until «Обновить»).
    Daily,
    /// A mood card: endless (it plays as a radio).
    Mood(Mood),
    /// «Твои миксы»: a style mix by its place in [`crate::HomeView::style_mixes`]
    /// (endless).
    Style(u8),
    /// «Миксы исполнителей»: the artist's radio.
    Artist(ArtistId),
    /// «Новое для тебя» → «Слушать всё»: the discoveries, then a radio of
    /// new music.
    Discover,
    /// «Дейлист»: the bento's big tile, picked anew for each part of the
    /// day and named by its sound and time («Тягучий вечер четверга»).
    /// Endless.
    Daylist,
    /// A smaller tile of the bento: one kind a day, picked anew every day
    /// ([`TileKind`], docs/stage4.md §12.9); it plays as it is.
    Tile(TileKind),
    /// «Миксы для тебя»: one moment, about 28 songs, the same all day
    /// (another with «Другой состав»); it plays as it is.
    Lane(MixKey),
}

impl MixId {
    /// The mix plays without end (a radio that extends itself); the others
    /// play their tracks as they are.
    pub const fn endless(self) -> bool {
        matches!(self, Self::Mood(_) | Self::Style(_) | Self::Artist(_) | Self::Discover | Self::Daylist)
    }
}

/// What a mix of «Миксы для тебя» is (the engine's own key, named here
/// for the view): a taste profile's mix by the profile's key, «Всё
/// вперемешку», the calm and the lively side of everything loved, a card of
/// «Фон», a blend of two or three profiles, a kind of song before there are
/// profiles, «На повторе»; a mix of the world's page open; the liked
/// songs' mix (not in the row: «Любимые» is the playlist's tile), the mix
/// of a part of the day (its index: morning, day, evening, night: «На
/// вечер»), the day's mixes — 0 is «Микс дня», from 1 the named mixes
/// («Микс: ZUTOMAYO») by their number — and the day's mix of one family of
/// genres («Хип-хоп микс») by the family's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum MixKey {
    Profile(u32),
    Everything,
    Calm,
    Lively,
    Background { profile: u32, card: u8, cards: u8 },
    Blend([u32; 3]),
    Kind(u8),
    OnRepeat,
    World(WorldMix),
    Liked,
    Part(u8),
    Daily(u8),
    Genre(u8),
}

impl MixKey {
    /// One of the day's mixes («Микс дня», a named mix, a genre mix): kept
    /// for the day.
    pub const fn is_daily(self) -> bool {
        matches!(self, Self::Daily(_) | Self::Genre(_))
    }
}

/// A mix of a world's page (recs-v2 §8): «Лучшее», «Ещё не слышал», «В
/// дорогу» (to other worlds), its calm and its lively side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum WorldMix {
    Best,
    Unheard,
    Road,
    Calm,
    Moving,
}

impl WorldMix {
    /// Its name on the world's page (the world's own is the page's title).
    pub const fn title(self) -> &'static str {
        match self {
            Self::Best => "Лучшее",
            Self::Unheard => "Ещё не слышал",
            Self::Road => "В дорогу",
            Self::Calm => "Тихое",
            Self::Moving => "В движении",
        }
    }
}

/// A mix of «Миксы для тебя».
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LaneCard {
    pub key: MixKey,
    /// «Русские песни», «Всё вперемешку», «Спокойное», «Фон».
    pub title: String,
    /// Shorter forms of the title, the longest first: a card too narrow
    /// for a blend's whole name says one of these («Русские и японские +
    /// без слов»), the whole in its tooltip.
    pub short_titles: Vec<String>,
    /// Its artists, most heard first, as the library writes them (the card
    /// says as many whole names as fit: «Серега Пират, mzlff и другие»).
    pub artists: Vec<String>,
    /// «С новым»: the artists of the new songs it mixes in.
    pub new_artists: Vec<String>,
    /// Four covers for the mosaic.
    pub covers: Vec<Cover>,
    /// «С новым»: the cover of a new song in the fourth place.
    pub new_cover: Option<Cover>,
    /// What it plays, in order.
    pub tracks: Vec<crate::TrackRow>,
    /// «С новым»: the new songs of the services it mixes in, in the order
    /// they play between its tracks.
    #[serde(default)]
    pub new_tracks: Vec<crate::StreamTrackRow>,
    /// The taste profile «Смешать с…» starts from (a profile's mix, «Фон»).
    pub blend_base: Option<u32>,
    /// The name the queue says when the title alone is not enough (a
    /// world's mix: «MADE IN ABYSS: лучшее»).
    pub queue: Option<String>,
    /// Why it is offered, in its tooltip (recs-v2 §12): «Из того, что ты
    /// дослушиваешь вечером».
    pub why: Option<String>,
}

impl LaneCard {
    pub const fn id(&self) -> MixId {
        MixId::Lane(self.key)
    }

    /// The name of what the queue plays from it.
    pub fn context(&self) -> String {
        format!("микса «{}»", self.queue.as_deref().unwrap_or(&self.title))
    }
}

/// A taste profile «Смешать с…» offers: its key and its name.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlendProfile {
    pub key: u32,
    pub title: String,
}

/// The kinds of the bento's smaller tiles (docs/stage4.md §12.9), in four
/// groups: memory, artists, sound, collection. A day shows «На повторе» and
/// a few others; the kicker and the title come with the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum TileKind {
    /// «На повторе»: the most completed of the last 30 days.
    OnRepeat,
    /// «Год назад»: played around this day a year ago.
    YearAgo,
    /// «Лето 2025»: the most played of a past season.
    Season,
    /// «Забытое любимое»: loved, not played for two months.
    Rediscover,
    /// «С самого начала»: the earliest likes.
    FirstLikes,
    /// «Свежие лайки»: liked in the last 30 days, newest first.
    FreshLikes,
    /// «Ждут своего часа»: added a week ago or more, never played.
    Unplayed,
    /// «Глубже у любимых»: barely played songs of the most played artists.
    DeepCuts,
    /// «Исполнитель дня»: the best of one of the top artists.
    ArtistOfDay,
    /// «Давно не слышал»: an artist played a lot, then not for 45 days.
    Comeback,
    /// «Потому что тебе понравилось»: what sounds like a recent like.
    BecauseYouLiked,
    /// «От 7 минут».
    Long,
    /// «До 2:30».
    Short,
    /// «Со словами»: songs with synced lyrics.
    SingAlong,
    /// «Японское», «По-русски»: by the script of the title and artist.
    Language,
    /// «Из 2000-х»: by the release year.
    Decade,
    /// «Made in Abyss»: the soundtracks of one franchise.
    Soundtrack,
    /// «Вечером»: the best of a playlist not opened for a while.
    Playlist,
    /// «Дослушай»: albums started and left before half.
    Unfinished,
    /// «Ещё не скачано»: liked stream songs without a file (↓ downloads
    /// them all, [`crate::Command::DownloadMix`]).
    NotDownloaded,
    /// «Лотерея»: anything, leaning to the rarely played.
    Lottery,
}

impl TileKind {
    pub const ALL: [Self; 21] = [
        Self::OnRepeat,
        Self::YearAgo,
        Self::Season,
        Self::Rediscover,
        Self::FirstLikes,
        Self::FreshLikes,
        Self::Unplayed,
        Self::DeepCuts,
        Self::ArtistOfDay,
        Self::Comeback,
        Self::BecauseYouLiked,
        Self::Long,
        Self::Short,
        Self::SingAlong,
        Self::Language,
        Self::Decade,
        Self::Soundtrack,
        Self::Playlist,
        Self::Unfinished,
        Self::NotDownloaded,
        Self::Lottery,
    ];

    /// The kind's name in screenshot steps and logs: `on_repeat`, `year_ago`…
    pub const fn key(self) -> &'static str {
        match self {
            Self::OnRepeat => "on_repeat",
            Self::YearAgo => "year_ago",
            Self::Season => "season",
            Self::Rediscover => "rediscover",
            Self::FirstLikes => "first_likes",
            Self::FreshLikes => "fresh_likes",
            Self::Unplayed => "unplayed",
            Self::DeepCuts => "deep_cuts",
            Self::ArtistOfDay => "artist_of_day",
            Self::Comeback => "comeback",
            Self::BecauseYouLiked => "because_you_liked",
            Self::Long => "long",
            Self::Short => "short",
            Self::SingAlong => "sing_along",
            Self::Language => "language",
            Self::Decade => "decade",
            Self::Soundtrack => "soundtrack",
            Self::Playlist => "playlist",
            Self::Unfinished => "unfinished",
            Self::NotDownloaded => "not_downloaded",
            Self::Lottery => "lottery",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }
}

/// One mix card of the home page.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MixCard {
    pub id: MixId,
    /// «Твой микс», «Спокойное», «mzlff, MORGENSHTERN и другие».
    pub title: String,
    /// The quiet label over a bento tile's title («ДЕЙЛИСТ», «ИСПОЛНИТЕЛЬ
    /// ДНЯ»); empty for the cards without one (moods, style mixes).
    pub kicker: String,
    /// A quiet second line: «30 треков · обновлён сегодня»; empty when the
    /// card has nothing to add.
    pub subtitle: String,
    /// Up to four covers for a collage (the first alone for a single
    /// cover); files of the library or a service's `https://` urls.
    pub covers: Vec<Cover>,
}

/// What a radio grows from ([`crate::Command::StartRadio`]): «Радио по
/// треку» from a library track or a service's track, «Радио» on an artist
/// page; «Радио по альбому» and «Радио по плейлисту» from a list of the
/// library or of a service.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RadioSeed {
    Track(TrackId),
    Stream(RemoteId),
    Artist(ArtistId),
    Album(crate::ReleaseId),
    Playlist(crate::PlaylistId),
    StreamAlbum(RemoteId),
    StreamPlaylist(RemoteId),
}

/// What «Не рекомендовать» is about ([`crate::Command::RecFeedback`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RecTarget {
    /// A library track (a liked stream track too).
    Track(TrackId),
    /// A library artist: none of their tracks is picked.
    Artist(ArtistId),
    /// A track of a service (a discovery): that song, on every service.
    StreamTrack(RemoteId),
    /// An artist of a service, by the name shown: none of their songs is
    /// discovered.
    StreamArtist(String),
}

/// The radio the queue plays ([`crate::QueueView::radio`]): it extends
/// itself while it plays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadioView {
    /// «Радио · Группа крови», «Радио · Кино», «Спокойное», «Твой микс».
    pub title: String,
    /// Looking for the next tracks right now.
    pub extending: bool,
    /// Started by «Автовоспроизведение» when the queue ran out (the title
    /// is then «Похожее на «…»»).
    pub autoplay: bool,
    /// What it grows from (the share of new music it remembers).
    pub kind: crate::RadioKind,
    /// «Моя волна» (or a card of «Миксы для тебя» started as one): its
    /// filters and the user's scenes to choose from.
    pub wave: Option<WaveView>,
}

/// «Новизна» of «Моя волна»: how much of it is new music.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WaveNovelty {
    /// «Баланс».
    #[default]
    Balance,
    /// «Больше нового».
    More,
    /// «Только новое».
    OnlyNew,
    /// «Знакомое»: the library alone.
    Familiar,
}

impl WaveNovelty {
    pub const ALL: [Self; 4] = [Self::Balance, Self::More, Self::OnlyNew, Self::Familiar];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Balance => "Баланс",
            Self::More => "Больше нового",
            Self::OnlyNew => "Только новое",
            Self::Familiar => "Знакомое",
        }
    }
}

/// «Характер» of «Моя волна».
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WaveCharacter {
    /// «Любой».
    #[default]
    Any,
    /// «Бодрое».
    Lively,
    /// «Спокойное».
    Calm,
    /// «Мрачное».
    Dark,
    /// «Фон».
    Background,
}

impl WaveCharacter {
    pub const ALL: [Self; 5] = [Self::Any, Self::Lively, Self::Calm, Self::Dark, Self::Background];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Any => "Любой",
            Self::Lively => "Бодрое",
            Self::Calm => "Спокойное",
            Self::Dark => "Мрачное",
            Self::Background => "Фон",
        }
    }
}

/// «Сцена» of «Моя волна»: everything, or one of the user's own scenes —
/// a taste profile by its key, a kind of song by its number (as
/// [`MixKey::Kind`]), a family of genres by its number (as
/// [`MixKey::Genre`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WaveScene {
    /// «Все».
    #[default]
    All,
    Profile(u32),
    Kind(u8),
    Genre(u8),
}

/// What «Моя волна» plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WaveFilter {
    pub novelty: WaveNovelty,
    pub character: WaveCharacter,
    pub scene: WaveScene,
}

/// One of the user's scenes «Моя волна» offers: «Хип-хоп», «Саундтреки»,
/// «Русские песни».
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WaveSceneCard {
    pub scene: WaveScene,
    pub title: String,
}

/// «Моя волна» playing ([`RadioView::wave`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaveView {
    pub filter: WaveFilter,
    /// The user's scenes, the largest first («Все» is not among them).
    pub scenes: Vec<WaveSceneCard>,
    /// The card of «Миксы для тебя» it was started from, if any.
    pub preset: Option<MixKey>,
}

