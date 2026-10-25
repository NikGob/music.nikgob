//! The recommendations part of the view model (stage 4, docs/stage4.md §6):
//! the mix cards of the home page, what plays a mix or a radio, the
//! «Не рекомендовать» feedback and the radio line of the queue.
//!
//! The engine itself (`nikgob-recs`) stays behind the runtime: the view only
//! names things ([`Mood`], [`MixId`]) and shows what the runtime filled in.

use crate::{ArtistId, Cover, RemoteId, TrackId};

/// The six moods of «Под настроение» (the engine's own enum stays in the
/// runtime; this is its name in the view).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// «Миксы для тебя»: the songs of one lane, about two hours, picked
    /// anew at 06:00 (or with «Собрать заново»); it plays as it is.
    Lane(Lane),
}

impl MixId {
    /// The mix plays without end (a radio that extends itself); the others
    /// play their tracks as they are.
    pub const fn endless(self) -> bool {
        matches!(self, Self::Mood(_) | Self::Style(_) | Self::Artist(_) | Self::Discover | Self::Daylist)
    }
}

/// The lanes of «Миксы для тебя»: how the user listens, by the language
/// of the songs and by soundtracks quiet or loud. The set and its order are
/// stable; only the songs change at 06:00.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Lane {
    /// «Песни на русском».
    Russian,
    /// «Тихие саундтреки».
    CalmSoundtracks,
    /// «Бодрые саундтреки».
    DriveSoundtracks,
    /// «Песни на английском».
    English,
    /// «Песни на японском» (a lane of its own with enough of them).
    Japanese,
    /// «Песни не на русском»: English and Japanese together, when Japanese
    /// is too little for a lane.
    NotRussian,
    /// «На повторе»: what played most lately.
    OnRepeat,
}

impl Lane {
    pub const ALL: [Self; 7] = [
        Self::Russian,
        Self::CalmSoundtracks,
        Self::DriveSoundtracks,
        Self::English,
        Self::Japanese,
        Self::NotRussian,
        Self::OnRepeat,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Russian => "Песни на русском",
            Self::CalmSoundtracks => "Тихие саундтреки",
            Self::DriveSoundtracks => "Бодрые саундтреки",
            Self::English => "Песни на английском",
            Self::Japanese => "Песни на японском",
            Self::NotRussian => "Песни не на русском",
            Self::OnRepeat => "На повторе",
        }
    }

    /// The lane's name in screenshot steps and logs.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Russian => "ru",
            Self::CalmSoundtracks => "calm",
            Self::DriveSoundtracks => "drive",
            Self::English => "en",
            Self::Japanese => "ja",
            Self::NotRussian => "foreign",
            Self::OnRepeat => "repeat",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.key() == key)
    }
}

/// A mix of «Миксы для тебя».
#[derive(Clone, Debug, PartialEq)]
pub struct LaneCard {
    pub lane: Lane,
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
}

impl LaneCard {
    pub const fn id(&self) -> MixId {
        MixId::Lane(self.lane)
    }
}

/// The kinds of the bento's smaller tiles (docs/stage4.md §12.9), in four
/// groups: memory, artists, sound, collection. A day shows «На повторе» and
/// a few others; the kicker and the title come with the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
#[derive(Clone, Debug, PartialEq)]
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
/// page.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RadioSeed {
    Track(TrackId),
    Stream(RemoteId),
    Artist(ArtistId),
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
}

