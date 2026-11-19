//! Home page, recommendations, auto-mixes, moods and wave strings.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{Locale, PerLocale, PluralForms, locale, plural_forms};

/// Static strings for the home page and recommendations.
pub struct HomeRecsStrings {
    pub my_wave: &'static str,
    pub listen_again: &'static str,
    pub discover: &'static str,
    pub mixes_for_you: &'static str,
    pub of_day: &'static str,
    pub your_mix: &'static str,
    pub daily_mix: &'static str,
    pub kicker_for_today: &'static str,
    /// «Следующий на альбоме»: why a radio's next pick is there.
    pub next_on_album: &'static str,
    /// «Похоже по звуку»: why a radio's pick was chosen.
    pub similar_sound: &'static str,
    /// «раз», «раза», «раз»: the times a song was played (see [`fmt_on_repeat_in`]).
    pub times: PluralForms,
}

pub static RU: HomeRecsStrings = HomeRecsStrings {
    my_wave: "Моя волна",
    listen_again: "Слушать снова",
    discover: "Новое для тебя",
    mixes_for_you: "Миксы для тебя",
    of_day: "Подборки дня",
    your_mix: "Твой микс",
    daily_mix: "Микс дня",
    kicker_for_today: "НА СЕГОДНЯ",
    next_on_album: "Следующий на альбоме",
    similar_sound: "Похоже по звуку",
    times: PluralForms { one: "раз", few: "раза", many: "раз" },
};

pub static EN: HomeRecsStrings = HomeRecsStrings {
    my_wave: "My Wave",
    listen_again: "Listen Again",
    discover: "Discover",
    mixes_for_you: "Mixes for you",
    of_day: "Daily picks",
    your_mix: "Your Mix",
    daily_mix: "Daily Mix",
    kicker_for_today: "For today",
    next_on_album: "Next on the album",
    similar_sound: "Similar sound",
    times: PluralForms { one: "time", few: "times", many: "times" },
};

locale_table! {
    /// Returns home/recommendation strings for `loc`.
    home_recs_in -> HomeRecsStrings,
    /// Returns home/recommendation strings for the active process locale.
    home_recs,
    { Ru => RU, En => EN }
}

/// Six mood cards in «Под настроение».
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MoodKind {
    Calm,
    Energetic,
    Dark,
    Bright,
    Focus,
    Drive,
}

impl MoodKind {
    /// Localized title of the mood card.
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Calm) => "Спокойное",
            (Locale::Ru, Self::Energetic) => "Энергичное",
            (Locale::Ru, Self::Dark) => "Мрачное",
            (Locale::Ru, Self::Bright) => "Светлое",
            (Locale::Ru, Self::Focus) => "Для фокуса",
            (Locale::Ru, Self::Drive) => "Драйв",
            (Locale::En, Self::Calm) => "Calm",
            (Locale::En, Self::Energetic) => "Energetic",
            (Locale::En, Self::Dark) => "Dark",
            (Locale::En, Self::Bright) => "Bright",
            (Locale::En, Self::Focus) => "Focus",
            (Locale::En, Self::Drive) => "Drive",
        }
    }
}

/// Background card variants in «Миксы для тебя».
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BackgroundVariant {
    Plain,
    Quiet,
    Lively,
}

impl BackgroundVariant {
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Plain) => "Фон",
            (Locale::Ru, Self::Quiet) => "Фон потише",
            (Locale::Ru, Self::Lively) => "Фон поживее",
            (Locale::En, Self::Plain) => "Background",
            (Locale::En, Self::Quiet) => "Quieter Background",
            (Locale::En, Self::Lively) => "Livelier Background",
        }
    }
}

/// Mix kinds on a world's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WorldMixKind {
    Best,
    Unheard,
    Road,
    Calm,
    Moving,
}

impl WorldMixKind {
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Best) => "Лучшее",
            (Locale::Ru, Self::Unheard) => "Ещё не слышал",
            (Locale::Ru, Self::Road) => "В дорогу",
            (Locale::Ru, Self::Calm) => "Тихое",
            (Locale::Ru, Self::Moving) => "В движении",
            (Locale::En, Self::Best) => "Best of",
            (Locale::En, Self::Unheard) => "Not played yet",
            (Locale::En, Self::Road) => "On the road",
            (Locale::En, Self::Calm) => "Calm",
            (Locale::En, Self::Moving) => "On the move",
        }
    }
}

/// Static tile titles for `TileKind` cards whose names do not depend on dynamic track data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TileStatic {
    OnRepeat,
    Rediscover,
    FirstLikes,
    FreshLikes,
    Unplayed,
    DeepCuts,
    Long,
    Short,
    SingAlong,
    Unfinished,
    NotDownloaded,
    Lottery,
}

impl TileStatic {
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::OnRepeat) => "На повторе",
            (Locale::Ru, Self::Rediscover) => "Забытое любимое",
            (Locale::Ru, Self::FirstLikes) => "Первые лайки",
            (Locale::Ru, Self::FreshLikes) => "Свежие лайки",
            (Locale::Ru, Self::Unplayed) => "Ждут своего часа",
            (Locale::Ru, Self::DeepCuts) => "Глубже у любимых",
            (Locale::Ru, Self::Long) => "Длинные треки",
            (Locale::Ru, Self::Short) => "Короткие треки",
            (Locale::Ru, Self::SingAlong) => "Со словами",
            (Locale::Ru, Self::Unfinished) => "Недослушанные альбомы",
            (Locale::Ru, Self::NotDownloaded) => "Ещё не скачано",
            (Locale::Ru, Self::Lottery) => "Лотерея",
            (Locale::En, Self::OnRepeat) => "On Repeat",
            (Locale::En, Self::Rediscover) => "Forgotten Favorites",
            (Locale::En, Self::FirstLikes) => "First Likes",
            (Locale::En, Self::FreshLikes) => "Recent Likes",
            (Locale::En, Self::Unplayed) => "Waiting to Play",
            (Locale::En, Self::DeepCuts) => "Artist Deep Cuts",
            (Locale::En, Self::Long) => "Long Tracks",
            (Locale::En, Self::Short) => "Short Tracks",
            (Locale::En, Self::SingAlong) => "With Lyrics",
            (Locale::En, Self::Unfinished) => "Unfinished Albums",
            (Locale::En, Self::NotDownloaded) => "Not Downloaded Yet",
            (Locale::En, Self::Lottery) => "Music Lottery",
        }
    }
}

/// Static kicker labels: the quiet line over a bento tile's title, one per
/// tile kind of `nikgob-recs::tiles::TileKind`, never the title again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TileKicker {
    OnRepeat,
    YearAgo,
    Season,
    Rediscover,
    FirstLikes,
    FreshLikes,
    Unplayed,
    DeepCuts,
    ArtistOfDay,
    Comeback,
    BecauseYouLiked,
    Long,
    Short,
    SingAlong,
    Language,
    Decade,
    Soundtrack,
    Playlist,
    Unfinished,
    NotDownloaded,
    Lottery,
}

impl TileKicker {
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::OnRepeat) => "Чаще всего",
            (Locale::Ru, Self::YearAgo) => "Год назад",
            (Locale::Ru, Self::Season) => "Капсула времени",
            (Locale::Ru, Self::Rediscover) => "Давно не играло",
            (Locale::Ru, Self::FirstLikes) => "С самого начала",
            (Locale::Ru, Self::FreshLikes) => "Недавно",
            (Locale::Ru, Self::Unplayed) => "Не играло",
            (Locale::Ru, Self::DeepCuts) => "Твои исполнители",
            (Locale::Ru, Self::ArtistOfDay) => "Исполнитель дня",
            (Locale::Ru, Self::Comeback) => "Давно не слышал",
            (Locale::Ru, Self::BecauseYouLiked) => "Тебе понравилось",
            (Locale::Ru, Self::Long) => "Длинное",
            (Locale::Ru, Self::Short) => "Короткое",
            (Locale::Ru, Self::SingAlong) => "Подпевай",
            (Locale::Ru, Self::Language) => "По языку",
            (Locale::Ru, Self::Decade) => "По годам",
            (Locale::Ru, Self::Soundtrack) => "Саундтрек",
            (Locale::Ru, Self::Playlist) => "Из плейлиста",
            (Locale::Ru, Self::Unfinished) => "Не дослушал",
            (Locale::Ru, Self::NotDownloaded) => "Не скачано",
            (Locale::Ru, Self::Lottery) => "Наугад",
            (Locale::En, Self::OnRepeat) => "Most played",
            (Locale::En, Self::YearAgo) => "A year ago",
            (Locale::En, Self::Season) => "Time capsule",
            (Locale::En, Self::Rediscover) => "Long unplayed",
            (Locale::En, Self::FirstLikes) => "From the start",
            (Locale::En, Self::FreshLikes) => "Recently",
            (Locale::En, Self::Unplayed) => "Never played",
            (Locale::En, Self::DeepCuts) => "Your artists",
            (Locale::En, Self::ArtistOfDay) => "Daily artist",
            (Locale::En, Self::Comeback) => "Long unheard",
            (Locale::En, Self::BecauseYouLiked) => "You liked",
            (Locale::En, Self::Long) => "Extended",
            (Locale::En, Self::Short) => "Brief",
            (Locale::En, Self::SingAlong) => "Sing along",
            (Locale::En, Self::Language) => "By language",
            (Locale::En, Self::Decade) => "By decade",
            (Locale::En, Self::Soundtrack) => "Soundtrack",
            (Locale::En, Self::Playlist) => "From a playlist",
            (Locale::En, Self::Unfinished) => "In progress",
            (Locale::En, Self::NotDownloaded) => "Streaming",
            (Locale::En, Self::Lottery) => "Random pick",
        }
    }
}

/// The script a song's name is written in: the «По языку» tile's static titles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScriptKind {
    Japanese,
    Korean,
    Cyrillic,
    Latin,
}

impl ScriptKind {
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Japanese) => "Японское",
            (Locale::Ru, Self::Korean) => "Корейское",
            (Locale::Ru, Self::Cyrillic) => "По-русски",
            (Locale::Ru, Self::Latin) => "Зарубежное",
            (Locale::En, Self::Japanese) => "Japanese",
            (Locale::En, Self::Korean) => "Korean",
            (Locale::En, Self::Cyrillic) => "Russian",
            (Locale::En, Self::Latin) => "International",
        }
    }
}

/// Localized month name (`month` 1..=12, wrapped out of range).
pub const fn month_name_in(loc: Locale, month: u32) -> &'static str {
    let m = (month.saturating_sub(1) % 12) as usize;
    match loc {
        Locale::Ru => [
            "Январь",
            "Февраль",
            "Март",
            "Апрель",
            "Май",
            "Июнь",
            "Июль",
            "Август",
            "Сентябрь",
            "Октябрь",
            "Ноябрь",
            "Декабрь",
        ][m],
        Locale::En => [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ][m],
    }
}

/// Localized season name (`season` 0 winter .. 3 autumn, wrapped out of range).
pub const fn season_name_in(loc: Locale, season: u32) -> &'static str {
    let s = (season % 4) as usize;
    match loc {
        Locale::Ru => ["Зима", "Весна", "Лето", "Осень"][s],
        Locale::En => ["Winter", "Spring", "Summer", "Autumn"][s],
    }
}

/// «Лето 2025» / «Summer 2025»: the season (`0` winter .. `3` autumn) of `year`.
pub fn fmt_season_in(loc: Locale, year: i32, season: u32) -> String {
    let name = season_name_in(loc, season);
    format!("{name} {year}")
}

/// «Из 90-х», «Из 2000-х» / «From the 90s», «From the 2000s».
pub fn fmt_decade_in(loc: Locale, decade: i32) -> String {
    match loc {
        Locale::Ru => {
            if (1900..2000).contains(&decade) {
                format!("Из {}-х", decade - 1900)
            } else {
                format!("Из {decade}-х")
            }
        }
        Locale::En => {
            if (1910..2000).contains(&decade) {
                format!("From the {}s", decade - 1900)
            } else {
                format!("From the {decade}s")
            }
        }
    }
}

/// «Похоже на Underground River» / «Similar to Underground River».
pub fn fmt_similar_to_in(loc: Locale, subject: &str) -> String {
    match loc {
        Locale::Ru => format!("Похоже на {subject}"),
        Locale::En => format!("Similar to {subject}"),
    }
}

/// Locale-neutral composition flavor of a Calm or Lively side mix (`MixKey::Calm` / `Lively`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SideMixFlavor {
    Russian,
    Japanese,
    Korean,
    English,
    Spanish,
    French,
    German,
    OtherLang,
    Instrumental,
    Songs,
    Plain,
}

impl SideMixFlavor {
    /// Renders a Calm or Lively side mix title in `loc`.
    pub fn title_in(self, calm: bool, loc: Locale) -> String {
        match loc {
            Locale::Ru => {
                let word = if calm { "потише" } else { "погромче" };
                match self {
                    Self::Russian => format!("Русские песни {word}"),
                    Self::Japanese => format!("Японские песни {word}"),
                    Self::Korean => format!("Корейские песни {word}"),
                    Self::English => format!("Английские песни {word}"),
                    Self::Spanish => format!("Испанские песни {word}"),
                    Self::French => format!("Французские песни {word}"),
                    Self::German => format!("Немецкие песни {word}"),
                    Self::OtherLang => format!("Песни {word}"),
                    Self::Instrumental => {
                        if calm { "Спокойное без слов" } else { "Бодрое без слов" }.to_owned()
                    }
                    Self::Songs => if calm { "Спокойные песни" } else { "Песни погромче" }.to_owned(),
                    Self::Plain => if calm { "Спокойное" } else { "Погромче" }.to_owned(),
                }
            }
            Locale::En => {
                let adj = if calm { "Calm" } else { "Upbeat" };
                match self {
                    Self::Russian => format!("{adj} Russian Songs"),
                    Self::Japanese => format!("{adj} Japanese Songs"),
                    Self::Korean => format!("{adj} Korean Songs"),
                    Self::English => format!("{adj} English Songs"),
                    Self::Spanish => format!("{adj} Spanish Songs"),
                    Self::French => format!("{adj} French Songs"),
                    Self::German => format!("{adj} German Songs"),
                    Self::OtherLang | Self::Songs => format!("{adj} Songs"),
                    Self::Instrumental => format!("{adj} Instrumental"),
                    Self::Plain => adj.to_owned(),
                }
            }
        }
    }
}

/// Typed identifier for auto-mix and card titles so language switching
/// updates cards and queue headers immediately without recomputing recs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MixTitle {
    YourMix,
    Daily,
    Everything,
    OnRepeat,
    Liked,
    Wave,
    Background(BackgroundVariant),
    DayPart(u8),
    Mood(MoodKind),
    World { world: Option<String>, mix: WorldMixKind },
    Genre(u8),
    Era(i32),
    Artist(String),
    Energy { calm: bool },
    Side { calm: bool, flavor: SideMixFlavor },
    Tile(TileStatic),
    BecauseYouLiked(String),
    Localized(PerLocale<String>),
    Custom(String),
}

impl MixTitle {
    /// Localized mix title for `loc`.
    pub fn title_in(&self, loc: Locale) -> Cow<'_, str> {
        match self {
            Self::YourMix => Cow::Borrowed(home_recs_in(loc).your_mix),
            Self::Daily => Cow::Borrowed(home_recs_in(loc).daily_mix),
            Self::Everything => Cow::Borrowed(match loc {
                Locale::Ru => "Всё вперемешку",
                Locale::En => "All Mixed",
            }),
            Self::OnRepeat => Cow::Borrowed(TileStatic::OnRepeat.title_in(loc)),
            Self::Liked => Cow::Borrowed(match loc {
                Locale::Ru => "Микс любимых",
                Locale::En => "Liked Mix",
            }),
            Self::Wave => Cow::Borrowed(home_recs_in(loc).my_wave),
            Self::Background(v) => Cow::Borrowed(v.title_in(loc)),
            Self::DayPart(p) => Cow::Borrowed(day_part_title_in(loc, *p)),
            Self::Mood(m) => Cow::Borrowed(m.title_in(loc)),
            Self::World { world, mix } => {
                let sub = mix.title_in(loc);
                match world {
                    Some(world) => {
                        let lower = match loc {
                            Locale::Ru => sub.to_lowercase(),
                            Locale::En => sub.to_owned(),
                        };
                        Cow::Owned(format!("{world}: {lower}"))
                    }
                    None => Cow::Borrowed(sub),
                }
            }
            Self::Genre(id) => match genre_family_title_in(loc, *id) {
                Some(t) => Cow::Borrowed(t),
                None => Cow::Borrowed(""),
            },
            Self::Era(decade) => Cow::Owned(match loc {
                Locale::Ru => {
                    if (1900..2000).contains(decade) {
                        format!("Микс {}-х", decade % 100)
                    } else {
                        format!("Микс {decade}-х")
                    }
                }
                Locale::En => {
                    if (1900..2000).contains(decade) {
                        format!("{}s Mix", decade % 100)
                    } else {
                        format!("{decade}s Mix")
                    }
                }
            }),
            Self::Artist(name) => Cow::Owned(match loc {
                Locale::Ru => format!("Микс: {name}"),
                Locale::En => format!("{name} Mix"),
            }),
            Self::Energy { calm } => Cow::Borrowed(match (loc, calm) {
                (Locale::Ru, true) => "Спокойный микс",
                (Locale::Ru, false) => "Бодрый микс",
                (Locale::En, true) => "Calm Mix",
                (Locale::En, false) => "Upbeat Mix",
            }),
            Self::Side { calm, flavor } => Cow::Owned(flavor.title_in(*calm, loc)),
            Self::Tile(t) => Cow::Borrowed(t.title_in(loc)),
            Self::BecauseYouLiked(track) => Cow::Owned(fmt_similar_to_in(loc, track)),
            Self::Localized(map) => Cow::Borrowed(map.get(loc).as_str()),
            Self::Custom(s) => Cow::Borrowed(s.as_str()),
        }
    }

    /// Card face title in `loc` (omits the world prefix already shown by the page).
    pub fn card_title_in(&self, loc: Locale) -> Cow<'_, str> {
        match self {
            Self::World { mix, .. } => Cow::Borrowed(mix.title_in(loc)),
            _ => self.title_in(loc),
        }
    }

    /// Card face title in the active process locale.
    pub fn card_title(&self) -> Cow<'_, str> {
        self.card_title_in(locale())
    }

    /// Localized mix title in the active process locale.
    pub fn title(&self) -> Cow<'_, str> {
        self.title_in(locale())
    }

    /// Parses a legacy Russian mix title string into a typed `MixTitle` when recognized.
    pub fn from_legacy(raw: &str) -> Self {
        let s = raw.trim();
        match s {
            "Твой микс" => Self::YourMix,
            "Микс дня" => Self::Daily,
            "Всё вперемешку" => Self::Everything,
            "На повторе" => Self::OnRepeat,
            "Микс любимых" => Self::Liked,
            "Моя волна" => Self::Wave,
            "Фон" => Self::Background(BackgroundVariant::Plain),
            "Фон потише" => Self::Background(BackgroundVariant::Quiet),
            "Фон поживее" => Self::Background(BackgroundVariant::Lively),
            "Доброе утро" => Self::DayPart(0),
            "День" => Self::DayPart(1),
            "Вечер" => Self::DayPart(2),
            "Ночь" => Self::DayPart(3),
            "Спокойное" => Self::Mood(MoodKind::Calm),
            "Энергичное" => Self::Mood(MoodKind::Energetic),
            "Мрачное" => Self::Mood(MoodKind::Dark),
            "Светлое" => Self::Mood(MoodKind::Bright),
            "Для фокуса" => Self::Mood(MoodKind::Focus),
            "Драйв" => Self::Mood(MoodKind::Drive),
            "Спокойный микс" => Self::Energy { calm: true },
            "Бодрый микс" => Self::Energy { calm: false },
            "Спокойное без слов" => Self::Side { calm: true, flavor: SideMixFlavor::Instrumental },
            "Бодрое без слов" => Self::Side { calm: false, flavor: SideMixFlavor::Instrumental },
            "Спокойные песни" => Self::Side { calm: true, flavor: SideMixFlavor::Songs },
            "Песни погромче" => Self::Side { calm: false, flavor: SideMixFlavor::Songs },
            "Погромче" => Self::Side { calm: false, flavor: SideMixFlavor::Plain },
            "Песни потише" => Self::Side { calm: true, flavor: SideMixFlavor::OtherLang },
            "Забытое любимое" => Self::Tile(TileStatic::Rediscover),
            "Первые лайки" => Self::Tile(TileStatic::FirstLikes),
            "Свежие лайки" => Self::Tile(TileStatic::FreshLikes),
            "Ждут своего часа" => Self::Tile(TileStatic::Unplayed),
            "Глубже у любимых" => Self::Tile(TileStatic::DeepCuts),
            "Длинные треки" => Self::Tile(TileStatic::Long),
            "Короткие треки" => Self::Tile(TileStatic::Short),
            "Со словами" | "Песни с текстом" => Self::Tile(TileStatic::SingAlong),
            "Недослушанные альбомы" => Self::Tile(TileStatic::Unfinished),
            "Ещё не скачано" => Self::Tile(TileStatic::NotDownloaded),
            "Лотерея" => Self::Tile(TileStatic::Lottery),
            _ => {
                if let Some((calm, flavor)) = parse_side_lang_ru(s) {
                    return Self::Side { calm, flavor };
                }
                if let Some(id) = genre_family_id_by_ru(s) {
                    return Self::Genre(id);
                }
                if let Some(artist) = s.strip_prefix("Микс: ") {
                    return Self::Artist(artist.to_owned());
                }
                if let Some(decade) = parse_era_ru(s) {
                    return Self::Era(decade);
                }
                if let Some((world, mix)) = parse_world_mix_ru(s) {
                    return Self::World {
                        world: Some(world.to_owned()),
                        mix,
                    };
                }
                if let Some(track) = s.strip_prefix("Похоже на «").and_then(|rest| rest.strip_suffix('»')) {
                    return Self::BecauseYouLiked(track.to_owned());
                }
                if let Some(track) = s.strip_prefix("Похоже на ") {
                    return Self::BecauseYouLiked(track.to_owned());
                }
                Self::Custom(s.to_owned())
            }
        }
    }
}

fn parse_side_lang_ru(s: &str) -> Option<(bool, SideMixFlavor)> {
    let (head, calm) = if let Some(h) = s.strip_suffix(" песни потише") {
        (h, true)
    } else if let Some(h) = s.strip_suffix(" песни погромче") {
        (h, false)
    } else {
        return None;
    };
    let flavor = match head {
        "Русские" => SideMixFlavor::Russian,
        "Японские" => SideMixFlavor::Japanese,
        "Корейские" => SideMixFlavor::Korean,
        "Английские" => SideMixFlavor::English,
        "Испанские" => SideMixFlavor::Spanish,
        "Французские" => SideMixFlavor::French,
        "Немецкие" => SideMixFlavor::German,
        _ => return None,
    };
    Some((calm, flavor))
}

fn parse_era_ru(s: &str) -> Option<i32> {
    let mid = s.strip_prefix("Микс ")?.strip_suffix("-х")?;
    let n: i32 = mid.parse().ok()?;
    if n < 0 || n % 10 != 0 {
        return None;
    }
    if n < 100 {
        Some(1900 + n)
    } else if (1900..=2090).contains(&n) {
        Some(n)
    } else {
        None
    }
}

fn parse_world_mix_ru(s: &str) -> Option<(&str, WorldMixKind)> {
    let (world, sub) = s.rsplit_once(": ")?;
    if world.is_empty() {
        return None;
    }
    let mix = match sub {
        "лучшее" | "Лучшее" => WorldMixKind::Best,
        "ещё не слышал" | "Ещё не слышал" => WorldMixKind::Unheard,
        "в дорогу" | "В дорогу" => WorldMixKind::Road,
        "тихое" | "Тихое" => WorldMixKind::Calm,
        "в движении" | "В движении" => WorldMixKind::Moving,
        _ => return None,
    };
    Some((world, mix))
}

/// Localized day-part mix title (`0` morning .. `3` night).
pub const fn day_part_title_in(loc: Locale, part: u8) -> &'static str {
    match (loc, part) {
        (Locale::Ru, 0) => "Доброе утро",
        (Locale::Ru, 1) => "День",
        (Locale::Ru, 2) => "Вечер",
        (Locale::Ru, _) => "Ночь",
        (Locale::En, 0) => "Good Morning",
        (Locale::En, 1) => "Daytime",
        (Locale::En, 2) => "Evening",
        (Locale::En, _) => "Night",
    }
}

/// Localized genre family mix title by family ID (`1..=23` from `nikgob-recs::mix_names`).
pub const fn genre_family_title_in(loc: Locale, id: u8) -> Option<&'static str> {
    Some(match loc {
        Locale::Ru => match id {
            1 => "J-pop микс",
            2 => "K-pop микс",
            3 => "J-rock микс",
            4 => "Пост-панк микс",
            5 => "Латинский микс",
            6 => "Регги-микс",
            7 => "Хип-хоп микс",
            8 => "Метал-микс",
            9 => "Панк-микс",
            10 => "Инди-микс",
            11 => "Рок-микс",
            12 => "Поп-микс",
            13 => "Микс саундтреков",
            14 => "Лоу-фай микс",
            15 => "Эмбиент-микс",
            16 => "Электронный микс",
            17 => "R&B микс",
            18 => "Джазовый микс",
            19 => "Блюзовый микс",
            20 => "Классический микс",
            21 => "Кантри-микс",
            22 => "Фолк-микс",
            23 => "Диско-микс",
            _ => return None,
        },
        Locale::En => match id {
            1 => "J-Pop Mix",
            2 => "K-Pop Mix",
            3 => "J-Rock Mix",
            4 => "Post-Punk Mix",
            5 => "Latin Mix",
            6 => "Reggae Mix",
            7 => "Hip-Hop Mix",
            8 => "Metal Mix",
            9 => "Punk Mix",
            10 => "Indie Mix",
            11 => "Rock Mix",
            12 => "Pop Mix",
            13 => "Soundtrack Mix",
            14 => "Lo-Fi Mix",
            15 => "Ambient Mix",
            16 => "Electronic Mix",
            17 => "R&B Mix",
            18 => "Jazz Mix",
            19 => "Blues Mix",
            20 => "Classical Mix",
            21 => "Country Mix",
            22 => "Folk Mix",
            23 => "Disco Mix",
            _ => return None,
        },
    })
}

fn genre_family_id_by_ru(title: &str) -> Option<u8> {
    (1..=23u8).find(|&id| genre_family_title_in(Locale::Ru, id) == Some(title))
}

// ------------------------------------------------------------ recipes' explanations

/// «утром», «днём», «вечером», «ночью» / «in the morning»…: the adverb of a
/// day part (`0` morning .. `3` night), as a moment's explanation line says it.
pub const fn day_part_adverb_in(loc: Locale, part: u8) -> &'static str {
    match (loc, part) {
        (Locale::Ru, 0) => "утром",
        (Locale::Ru, 1) => "днём",
        (Locale::Ru, 2) => "вечером",
        (Locale::Ru, _) => "ночью",
        (Locale::En, 0) => "in the morning",
        (Locale::En, 1) => "during the day",
        (Locale::En, 2) => "in the evening",
        (Locale::En, _) => "at night",
    }
}

/// «Из того, что ты дослушиваешь вечером, — без слов» / «Tracks you finish
/// listening to in the evening — instrumental» (see `nikgob_recs::explain`).
/// `part`: the day part (`0` morning .. `3` night) the songs are heard out in.
pub fn fmt_moment_in(loc: Locale, part: Option<u8>, instrumental: bool) -> String {
    let mut line = match loc {
        Locale::Ru => "Из того, что ты дослушиваешь".to_owned(),
        Locale::En => "Tracks you finish listening to".to_owned(),
    };
    if let Some(part) = part {
        line.push(' ');
        line.push_str(day_part_adverb_in(loc, part));
    }
    if instrumental {
        match loc {
            Locale::Ru => line.push_str(", — без слов"),
            Locale::En => line.push_str(" — instrumental"),
        }
    }
    line
}

/// «Новое: нашлось по Ghost Avenue в YouTube Music» / «New: found via Ghost Avenue on YouTube Music».
pub fn fmt_new_found_in(loc: Locale, seed: &str, service: &str) -> String {
    match loc {
        Locale::Ru => format!("Новое: нашлось по {seed} в {service}"),
        Locale::En => format!("New: found via {seed} on {service}"),
    }
}

/// «Новое: советует Deezer» / «New: suggested by Deezer».
pub fn fmt_new_suggested_in(loc: Locale, service: &str) -> String {
    match loc {
        Locale::Ru => format!("Новое: советует {service}"),
        Locale::En => format!("New: suggested by {service}"),
    }
}

/// «Ты включал это 12 раз за неделю» / «You played this 12 times this week».
pub fn fmt_on_repeat_in(loc: Locale, starts: u32) -> String {
    let unit = plural_forms(loc, u64::from(starts), &home_recs_in(loc).times);
    match loc {
        Locale::Ru => format!("Ты включал это {starts} {unit} за неделю"),
        Locale::En => format!("You played this {starts} {unit} this week"),
    }
}

/// «Что ты дослушивал чаще всего в MADE IN ABYSS» / «What you finished most in MADE IN ABYSS».
pub fn fmt_world_best_in(loc: Locale, world: &str) -> String {
    match loc {
        Locale::Ru => format!("Что ты дослушивал чаще всего в {world}"),
        Locale::En => format!("Tracks you most often play to the end in {world}"),
    }
}

/// «Ты не слышал; по звуку ближе всего к Chain of the Abyss» / «Unheard; closest in sound to Chain of the Abyss».
pub fn fmt_world_unheard_near_in(loc: Locale, track: &str) -> String {
    match loc {
        Locale::Ru => format!("Ты не слышал; по звуку ближе всего к {track}"),
        Locale::En => format!("Not played yet; sounds closest to {track}"),
    }
}

/// «Ты не слышал ничего из этого» / «None of these heard yet».
pub const fn world_unheard_none_in(loc: Locale) -> &'static str {
    match loc {
        Locale::Ru => "Ты не слышал ничего из этого",
        Locale::En => "You haven't heard any of these yet",
    }
}

/// «Звучит как твой MADE IN ABYSS, но из других миров» / «Sounds like your MADE IN ABYSS, from other worlds».
pub fn fmt_world_road_in(loc: Locale, world: &str) -> String {
    match loc {
        Locale::Ru => format!("Звучит как твой {world}, но из других миров"),
        Locale::En => format!("Sounds like your {world}, from other worlds"),
    }
}

/// «Следующий на альбоме» / «Next on album», both locales.
pub fn next_on_album_pair() -> PerLocale<String> {
    PerLocale {
        ru: home_recs_in(Locale::Ru).next_on_album.to_owned(),
        en: home_recs_in(Locale::En).next_on_album.to_owned(),
    }
}

/// «После Рассел кроу: часто идёт следом» / «Often follows Рассел кроу», both locales.
pub fn follows_often_pair(name: &str) -> PerLocale<String> {
    PerLocale {
        ru: format!("После {name}: часто идёт следом"),
        en: format!("Often follows {name}"),
    }
}

/// «Часто слушают вместе с Рассел кроу» / «Often played with Рассел кроу», both locales.
pub fn listened_together_pair(name: &str) -> PerLocale<String> {
    PerLocale {
        ru: format!("Часто слушают вместе с {name}"),
        en: format!("Often played with {name}"),
    }
}

/// «Похоже по звуку» / «Similar sound», both locales.
pub fn similar_sound_pair() -> PerLocale<String> {
    PerLocale {
        ru: home_recs_in(Locale::Ru).similar_sound.to_owned(),
        en: home_recs_in(Locale::En).similar_sound.to_owned(),
    }
}

// ------------------------------------------------------------ taste profiles

/// The sung languages a taste profile is named by (the named ones of
/// `nikgob-recs`'s `Lang`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SungLang {
    Russian,
    Japanese,
    Korean,
    English,
    Spanish,
    French,
    German,
}

impl SungLang {
    /// The adjective of this language's songs in `loc` («Русские» / «Russian»).
    pub const fn adjective_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Russian) => "Русские",
            (Locale::Ru, Self::Japanese) => "Японские",
            (Locale::Ru, Self::Korean) => "Корейские",
            (Locale::Ru, Self::English) => "Английские",
            (Locale::Ru, Self::Spanish) => "Испанские",
            (Locale::Ru, Self::French) => "Французские",
            (Locale::Ru, Self::German) => "Немецкие",
            (Locale::En, Self::Russian) => "Russian",
            (Locale::En, Self::Japanese) => "Japanese",
            (Locale::En, Self::Korean) => "Korean",
            (Locale::En, Self::English) => "English",
            (Locale::En, Self::Spanish) => "Spanish",
            (Locale::En, Self::French) => "French",
            (Locale::En, Self::German) => "German",
        }
    }
}

/// «Русские песни» / «Russian Songs»: a language's adjective and the songs.
pub fn fmt_lang_songs_in(loc: Locale, adjective: &str) -> String {
    match loc {
        Locale::Ru => format!("{adjective} песни"),
        Locale::En => format!("{adjective} Songs"),
    }
}

/// The kind of song a taste profile is named by (see `nikgob-recs`'s `Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProfileKind {
    Instrumental,
    InstrumentalSoundtracks,
    Songs,
    Soundtracks,
    Calm,
    Lively,
}

impl ProfileKind {
    /// Localized name of the profile kind.
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Instrumental) => "Без слов",
            (Locale::Ru, Self::InstrumentalSoundtracks) => "Саундтреки без слов",
            (Locale::Ru, Self::Songs) => "Песни",
            (Locale::Ru, Self::Soundtracks) => "Саундтреки",
            (Locale::Ru, Self::Calm) => "Спокойное",
            (Locale::Ru, Self::Lively) => "Бодрое",
            (Locale::En, Self::Instrumental) => "Instrumental",
            (Locale::En, Self::InstrumentalSoundtracks) => "Instrumental Soundtracks",
            (Locale::En, Self::Songs) => "Songs",
            (Locale::En, Self::Soundtracks) => "Soundtracks",
            (Locale::En, Self::Calm) => "Calm",
            (Locale::En, Self::Lively) => "Upbeat",
        }
    }
}

/// «потише» / «погромче» / «quieter» / «louder»: the loudness word a taste
/// profile's name is told apart by.
pub const fn loudness_word_in(loc: Locale, quiet: bool) -> &'static str {
    match (loc, quiet) {
        (Locale::Ru, true) => "потише",
        (Locale::Ru, false) => "погромче",
        (Locale::En, true) => "quieter",
        (Locale::En, false) => "louder",
    }
}

/// «Eve и Radwimps» / «Eve & Radwimps»: two artists of a profile's name.
pub fn fmt_artists_pair_in(loc: Locale, a: &str, b: &str) -> String {
    match loc {
        Locale::Ru => format!("{a} и {b}"),
        Locale::En => format!("{a} & {b}"),
    }
}

/// «Eve, Radwimps и другие» / «Eve, Radwimps & more»: a profile's artists.
pub fn fmt_artists_more_in(loc: Locale, a: &str, b: &str) -> String {
    match loc {
        Locale::Ru => format!("{a}, {b} и другие"),
        Locale::En => format!("{a}, {b} & more"),
    }
}

// ------------------------------------------------------------ daylists

/// The gender of the Russian noun of a day part (see [`daylist_name_in`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayGender {
    Masculine,
    Neuter,
    Feminine,
}

/// Daylist adjectives (Russian, masculine) by the cell of the axes: index
/// `energy ≥ 0` × 4 + `brightness ≥ 0` × 2 + `steadiness ≥ 0`.
pub const DAYLIST_ADJECTIVES_RU: [&[&str]; 8] = [
    // Quiet, dark, restless.
    &["Мрачный", "Туманный", "Сумрачный"],
    // Quiet, dark, steady.
    &["Тягучий", "Тихий", "Глубокий"],
    // Quiet, bright, restless.
    &["Мечтательный", "Воздушный", "Лёгкий"],
    // Quiet, bright, steady.
    &["Ясный", "Нежный", "Тёплый"],
    // Loud, dark, restless.
    &["Бурный", "Мятежный", "Колючий"],
    // Loud, dark, steady.
    &["Напористый", "Хлёсткий", "Упрямый"],
    // Loud, bright, restless.
    &["Яркий", "Озорной", "Пёстрый"],
    // Loud, bright, steady.
    &["Бодрый", "Солнечный", "Звонкий"],
];

/// English daylist adjectives (the same 8-cell grid as [`DAYLIST_ADJECTIVES_RU`]).
pub const DAYLIST_ADJECTIVES_EN: [&[&str]; 8] = [
    // Quiet, dark, restless.
    &["Gloomy", "Misty", "Dusky"],
    // Quiet, dark, steady.
    &["Mellow", "Quiet", "Deep"],
    // Quiet, bright, restless.
    &["Dreamy", "Airy", "Light"],
    // Quiet, bright, steady.
    &["Serene", "Gentle", "Warm"],
    // Loud, dark, restless.
    &["Stormy", "Rebellious", "Edgy"],
    // Loud, dark, steady.
    &["Driven", "Punchy", "Fierce"],
    // Loud, bright, restless.
    &["Vibrant", "Playful", "Vivid"],
    // Loud, bright, steady.
    &["Upbeat", "Sunny", "Crisp"],
];

/// «понедельника» … «воскресенья»: the weekday of a local day number (day 0
/// was a Thursday) in the Russian genitive.
pub fn weekday_genitive_ru(day: i64) -> &'static str {
    const NAMES: [&str; 7] = ["понедельника", "вторника", "среды", "четверга", "пятницы", "субботы", "воскресенья"];
    NAMES[(day + 3).rem_euclid(7) as usize]
}

/// «Monday» … «Sunday»: the English weekday of a local day number (day 0
/// was a Thursday).
pub fn weekday_en(day: i64) -> &'static str {
    const NAMES: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    NAMES[(day + 3).rem_euclid(7) as usize]
}

/// The Russian noun of a day part (`0` morning .. `3` night) and its gender.
pub const fn day_part_noun_ru(part: u8) -> (&'static str, DayGender) {
    match part {
        0 => ("утро", DayGender::Neuter),
        1 => ("день", DayGender::Masculine),
        2 => ("вечер", DayGender::Masculine),
        _ => ("ночь", DayGender::Feminine),
    }
}

/// The English noun of a day part (`0` morning .. `3` night).
pub const fn day_part_noun_en(part: u8) -> &'static str {
    match part {
        0 => "Morning",
        1 => "Afternoon",
        2 => "Evening",
        _ => "Night",
    }
}

/// A Russian masculine adjective («-ый», «-ой», «-ий») in `gender`.
pub fn agree_ru(masculine: &str, gender: DayGender) -> String {
    if gender == DayGender::Masculine {
        return masculine.to_owned();
    }
    let mut chars: Vec<char> = masculine.chars().collect();
    let ending: String = chars.split_off(chars.len().saturating_sub(2)).into_iter().collect();
    let stem: String = chars.iter().collect();
    let last = chars.last().copied().unwrap_or(' ');
    let (neuter, feminine) = match ending.as_str() {
        "ий" if "жчшщ".contains(last) => ("ее", "ая"),
        "ий" if "гкх".contains(last) => ("ое", "ая"),
        // A soft stem («синий»).
        "ий" => ("ее", "яя"),
        _ => ("ое", "ая"),
    };
    let ending = if gender == DayGender::Neuter { neuter } else { feminine };
    format!("{stem}{ending}")
}

/// «Тягучий вечер четверга» / «Mellow Thursday Evening»: a daylist's name in
/// `loc` from its adjective `cell` of the axes, the adjective `pick` of that
/// cell, its day part (`0` morning .. `3` night) and the local day number.
pub fn daylist_name_in(loc: Locale, cell: usize, pick: usize, part: u8, day: i64) -> String {
    let adjectives = match loc {
        Locale::Ru => &DAYLIST_ADJECTIVES_RU,
        Locale::En => &DAYLIST_ADJECTIVES_EN,
    };
    match loc {
        Locale::Ru => {
            let (noun, gender) = day_part_noun_ru(part);
            format!("{} {noun} {}", agree_ru(adjectives[cell][pick], gender), weekday_genitive_ru(day))
        }
        Locale::En => format!("{} {} {}", DAYLIST_ADJECTIVES_EN[cell][pick], weekday_en(day), day_part_noun_en(part)),
    }
}

// ------------------------------------------------------------ style mixes

/// A dominant sound trait of a style mix (see `nikgob-recs`'s `StyleWord`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleTrait {
    Energetic,
    Calm,
    Bright,
    Dark,
    Steady,
    Shifting,
}

impl StyleTrait {
    /// Localized descriptor word of the trait in `loc`.
    pub const fn word_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::Energetic) => "энергичное",
            (Locale::Ru, Self::Calm) => "спокойное",
            (Locale::Ru, Self::Bright) => "светлое",
            (Locale::Ru, Self::Dark) => "мрачное",
            (Locale::Ru, Self::Steady) => "ровное",
            (Locale::Ru, Self::Shifting) => "переменчивое",
            (Locale::En, Self::Energetic) => "energetic",
            (Locale::En, Self::Calm) => "calm",
            (Locale::En, Self::Bright) => "bright",
            (Locale::En, Self::Dark) => "dark",
            (Locale::En, Self::Steady) => "steady",
            (Locale::En, Self::Shifting) => "shifting",
        }
    }
}

/// «спокойное и мрачное» / «calm and dark»: two dominant sound traits of a style mix.
pub fn fmt_style_words_in(loc: Locale, a: StyleTrait, b: StyleTrait) -> String {
    match loc {
        Locale::Ru => format!("{} и {}", a.word_in(loc), b.word_in(loc)),
        Locale::En => format!("{} and {}", a.word_in(loc), b.word_in(loc)),
    }
}

// ------------------------------------------------------------ blends

/// The short word of a taste profile's kind in a blend's shortened name
/// (see `nikgob-recs`'s `Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendWord {
    /// Both instrumental kinds: «без слов» / «instrumental».
    Wordless,
    Soundtracks,
    Songs,
    Calm,
    Upbeat,
}

/// «без слов», «саундтреки», «песни», «спокойное», «бодрое» / «instrumental»,
/// «soundtracks», «songs», «calm», «upbeat».
pub const fn blend_word_in(loc: Locale, word: BlendWord) -> &'static str {
    match (loc, word) {
        (Locale::Ru, BlendWord::Wordless) => "без слов",
        (Locale::Ru, BlendWord::Soundtracks) => "саундтреки",
        (Locale::Ru, BlendWord::Songs) => "песни",
        (Locale::Ru, BlendWord::Calm) => "спокойное",
        (Locale::Ru, BlendWord::Upbeat) => "бодрое",
        (Locale::En, BlendWord::Wordless) => "instrumental",
        (Locale::En, BlendWord::Soundtracks) => "soundtracks",
        (Locale::En, BlendWord::Songs) => "songs",
        (Locale::En, BlendWord::Calm) => "calm",
        (Locale::En, BlendWord::Upbeat) => "upbeat",
    }
}

/// «песни» / «Songs»: the noun after a blend's language adjectives.
pub const fn blend_songs_noun_in(loc: Locale) -> &'static str {
    match loc {
        Locale::Ru => "песни",
        Locale::En => "Songs",
    }
}

/// Joins adjectives of one noun: «Русские, японские и английские» /
/// «Russian, Japanese & English» (the first as given, the rest lower case in Russian).
pub fn join_adjectives_in(loc: Locale, adjectives: &[&str]) -> String {
    match loc {
        Locale::Ru => {
            let mut out = adjectives[0].to_owned();
            for (k, a) in adjectives.iter().enumerate().skip(1) {
                out.push_str(if k + 1 == adjectives.len() { " и " } else { ", " });
                out.push_str(&(*a).to_lowercase());
            }
            out
        }
        Locale::En => match adjectives {
            [] => String::new(),
            [a] => (*a).to_owned(),
            [a, b] => format!("{a} & {b}"),
            [init @ .., last] => format!("{} & {last}", init.join(", ")),
        },
    }
}

/// «Русские и японские» / «Russian & Japanese»: the adjectives of one noun
/// in a blend's name (the first as given, the rest lower case in Russian,
/// joined by «и»).
pub fn blend_adjectives_in(loc: Locale, adjectives: &[&str]) -> String {
    match loc {
        Locale::Ru => {
            let mut first = (*adjectives[0]).to_owned();
            let rest: Vec<String> = adjectives[1..].iter().map(|a| (*a).to_lowercase()).collect();
            first.push_str(&format!(" и {}", rest.join(" и ")));
            first
        }
        Locale::En => join_adjectives_in(loc, adjectives),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_helpers_format_months_seasons_decades_and_likes() {
        assert_eq!(month_name_in(Locale::Ru, 1), "Январь");
        assert_eq!(month_name_in(Locale::Ru, 10), "Октябрь");
        assert_eq!(month_name_in(Locale::Ru, 12), "Декабрь");
        assert_eq!(month_name_in(Locale::Ru, 13), "Январь");
        assert_eq!(month_name_in(Locale::En, 10), "October");
        assert_eq!(fmt_season_in(Locale::Ru, 2025, 2), "Лето 2025");
        assert_eq!(fmt_season_in(Locale::Ru, 2025, 0), "Зима 2025");
        assert_eq!(fmt_season_in(Locale::En, 2025, 2), "Summer 2025");
        assert_eq!(fmt_season_in(Locale::En, 2025, 0), "Winter 2025");
        assert_eq!(fmt_decade_in(Locale::Ru, 1990), "Из 90-х");
        assert_eq!(fmt_decade_in(Locale::Ru, 2000), "Из 2000-х");
        assert_eq!(fmt_decade_in(Locale::En, 1990), "From the 90s");
        assert_eq!(fmt_decade_in(Locale::En, 2000), "From the 2000s");
        assert_eq!(fmt_similar_to_in(Locale::Ru, "Underground River"), "Похоже на Underground River");
        assert_eq!(fmt_similar_to_in(Locale::En, "Underground River"), "Similar to Underground River");
    }

    #[test]
    fn tile_kickers_and_script_kinds_translate() {
        assert_eq!(TileKicker::OnRepeat.title_in(Locale::Ru), "Чаще всего");
        assert_eq!(TileKicker::OnRepeat.title_in(Locale::En), "Most played");
        assert_eq!(TileKicker::YearAgo.title_in(Locale::Ru), "Год назад");
        assert_eq!(TileKicker::YearAgo.title_in(Locale::En), "A year ago");
        assert_eq!(TileKicker::Lottery.title_in(Locale::Ru), "Наугад");
        assert_eq!(TileKicker::Lottery.title_in(Locale::En), "Random pick");
        assert_eq!(ScriptKind::Japanese.title_in(Locale::Ru), "Японское");
        assert_eq!(ScriptKind::Cyrillic.title_in(Locale::Ru), "По-русски");
        assert_eq!(ScriptKind::Latin.title_in(Locale::En), "International");
        for loc in Locale::ALL {
            for k in [
                TileKicker::OnRepeat,
                TileKicker::YearAgo,
                TileKicker::Season,
                TileKicker::Rediscover,
                TileKicker::FirstLikes,
                TileKicker::FreshLikes,
                TileKicker::Unplayed,
                TileKicker::DeepCuts,
                TileKicker::ArtistOfDay,
                TileKicker::Comeback,
                TileKicker::BecauseYouLiked,
                TileKicker::Long,
                TileKicker::Short,
                TileKicker::SingAlong,
                TileKicker::Language,
                TileKicker::Decade,
                TileKicker::Soundtrack,
                TileKicker::Playlist,
                TileKicker::Unfinished,
                TileKicker::NotDownloaded,
                TileKicker::Lottery,
            ] {
                assert!(!k.title_in(loc).is_empty(), "{loc:?} {k:?}");
            }
        }
    }

    #[test]
    fn explanation_and_profile_strings_translate() {
        assert_eq!(fmt_new_found_in(Locale::Ru, "Ghost Avenue", "YouTube Music"), "Новое: нашлось по Ghost Avenue в YouTube Music");
        assert_eq!(fmt_new_found_in(Locale::En, "Ghost Avenue", "YouTube Music"), "New: found via Ghost Avenue on YouTube Music");
        assert_eq!(fmt_new_suggested_in(Locale::Ru, "Deezer"), "Новое: советует Deezer");
        assert_eq!(fmt_new_suggested_in(Locale::En, "Deezer"), "New: suggested by Deezer");
        // Plural cases: one / few / many.
        assert_eq!(fmt_on_repeat_in(Locale::Ru, 3), "Ты включал это 3 раза за неделю");
        assert_eq!(fmt_on_repeat_in(Locale::Ru, 12), "Ты включал это 12 раз за неделю");
        assert_eq!(fmt_on_repeat_in(Locale::Ru, 22), "Ты включал это 22 раза за неделю");
        assert_eq!(fmt_on_repeat_in(Locale::En, 2), "You played this 2 times this week");
        assert_eq!(fmt_on_repeat_in(Locale::En, 1), "You played this 1 time this week");
        assert_eq!(fmt_moment_in(Locale::Ru, Some(2), true), "Из того, что ты дослушиваешь вечером, — без слов");
        assert_eq!(fmt_moment_in(Locale::En, Some(2), true), "Tracks you finish listening to in the evening — instrumental");
        assert_eq!(fmt_moment_in(Locale::Ru, None, false), "Из того, что ты дослушиваешь");
        assert_eq!(fmt_world_best_in(Locale::Ru, "Мир"), "Что ты дослушивал чаще всего в Мир");
        assert_eq!(fmt_world_best_in(Locale::En, "World"), "Tracks you most often play to the end in World");
        assert_eq!(fmt_world_unheard_near_in(Locale::Ru, "T1"), "Ты не слышал; по звуку ближе всего к T1");
        assert_eq!(fmt_world_unheard_near_in(Locale::En, "T1"), "Not played yet; sounds closest to T1");
        assert_eq!(world_unheard_none_in(Locale::Ru), "Ты не слышал ничего из этого");
        assert_eq!(world_unheard_none_in(Locale::En), "You haven't heard any of these yet");
        assert_eq!(fmt_world_road_in(Locale::Ru, "Мир"), "Звучит как твой Мир, но из других миров");
        assert_eq!(fmt_world_road_in(Locale::En, "World"), "Sounds like your World, from other worlds");
        assert_eq!(next_on_album_pair().ru, "Следующий на альбоме");
        assert_eq!(next_on_album_pair().en, "Next on the album");
        assert_eq!(follows_often_pair("Рассел кроу").ru, "После Рассел кроу: часто идёт следом");
        assert_eq!(follows_often_pair("R").en, "Often follows R");
        assert_eq!(listened_together_pair("Рассел кроу").ru, "Часто слушают вместе с Рассел кроу");
        assert_eq!(listened_together_pair("R").en, "Often played with R");
        assert_eq!(similar_sound_pair().ru, "Похоже по звуку");
        assert_eq!(similar_sound_pair().en, "Similar sound");
        assert_eq!(ProfileKind::InstrumentalSoundtracks.title_in(Locale::Ru), "Саундтреки без слов");
        assert_eq!(ProfileKind::Lively.title_in(Locale::En), "Upbeat");
        assert_eq!(SungLang::Russian.adjective_in(Locale::Ru), "Русские");
        assert_eq!(SungLang::German.adjective_in(Locale::En), "German");
        assert_eq!(fmt_lang_songs_in(Locale::Ru, "Русские"), "Русские песни");
        assert_eq!(fmt_lang_songs_in(Locale::En, "Russian"), "Russian Songs");
        assert_eq!(loudness_word_in(Locale::Ru, true), "потише");
        assert_eq!(loudness_word_in(Locale::En, false), "louder");
        assert_eq!(fmt_artists_pair_in(Locale::Ru, "Eve", "Radwimps"), "Eve и Radwimps");
        assert_eq!(fmt_artists_pair_in(Locale::En, "Eve", "Radwimps"), "Eve & Radwimps");
        assert_eq!(fmt_artists_more_in(Locale::Ru, "Eve", "Radwimps"), "Eve, Radwimps и другие");
        assert_eq!(fmt_artists_more_in(Locale::En, "Eve", "Radwimps"), "Eve, Radwimps & more");
    }

    #[test]
    fn daylist_style_and_blend_names_translate() {
        // Day 0 was a Thursday; quiet, dark, steady is cell 1.
        assert_eq!(daylist_name_in(Locale::Ru, 1, 0, 1, 0), "Тягучий день четверга");
        assert!(daylist_name_in(Locale::En, 1, 0, 1, 0).ends_with(" Thursday Afternoon"));
        let morning = daylist_name_in(Locale::Ru, 6, 0, 0, 4);
        assert!(morning.ends_with("ое утро понедельника"), "{morning}");
        assert_eq!(weekday_genitive_ru(0), "четверга");
        assert_eq!(weekday_en(0), "Thursday");
        assert_eq!(agree_ru("Синий", DayGender::Feminine), "Синяя");
        for (ru, en) in DAYLIST_ADJECTIVES_RU.iter().zip(&DAYLIST_ADJECTIVES_EN) {
            assert_eq!(ru.len(), en.len());
        }
        assert_eq!(fmt_style_words_in(Locale::Ru, StyleTrait::Calm, StyleTrait::Dark), "спокойное и мрачное");
        assert_eq!(fmt_style_words_in(Locale::En, StyleTrait::Shifting, StyleTrait::Energetic), "shifting and energetic");
        assert_eq!(blend_word_in(Locale::Ru, BlendWord::Wordless), "без слов");
        assert_eq!(blend_word_in(Locale::En, BlendWord::Upbeat), "upbeat");
        assert_eq!(blend_songs_noun_in(Locale::Ru), "песни");
        assert_eq!(blend_songs_noun_in(Locale::En), "Songs");
        assert_eq!(join_adjectives_in(Locale::Ru, &["Russian", "Japanese", "English"]), "Russian, japanese и english");
        assert_eq!(join_adjectives_in(Locale::En, &["Russian", "Japanese", "English"]), "Russian, Japanese & English");
        assert_eq!(blend_adjectives_in(Locale::Ru, &["Русские", "Японские"]), "Русские и японские");
        assert_eq!(blend_adjectives_in(Locale::En, &["Russian", "Japanese"]), "Russian & Japanese");
    }
}
