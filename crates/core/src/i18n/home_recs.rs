//! Home page, recommendations, auto-mixes, moods and wave strings.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{Locale, PerLocale, locale};

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
};

pub static EN: HomeRecsStrings = HomeRecsStrings {
    my_wave: "My Wave",
    listen_again: "Listen Again",
    discover: "Discover",
    mixes_for_you: "Mixes for You",
    of_day: "Daily Picks",
    your_mix: "Your Mix",
    daily_mix: "Daily Mix",
    kicker_for_today: "FOR TODAY",
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
            (Locale::En, Self::Unheard) => "Unheard",
            (Locale::En, Self::Road) => "Road Trip",
            (Locale::En, Self::Calm) => "Calm",
            (Locale::En, Self::Moving) => "In Motion",
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
            (Locale::En, Self::Rediscover) => "Forgotten Faves",
            (Locale::En, Self::FirstLikes) => "First Likes",
            (Locale::En, Self::FreshLikes) => "Fresh Likes",
            (Locale::En, Self::Unplayed) => "Waiting to Play",
            (Locale::En, Self::DeepCuts) => "Artist Deep Cuts",
            (Locale::En, Self::Long) => "Long Tracks",
            (Locale::En, Self::Short) => "Short Tracks",
            (Locale::En, Self::SingAlong) => "With Lyrics",
            (Locale::En, Self::Unfinished) => "Unfinished Albums",
            (Locale::En, Self::NotDownloaded) => "Not Downloaded",
            (Locale::En, Self::Lottery) => "Music Lottery",
        }
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
            Self::BecauseYouLiked(track) => Cow::Owned(match loc {
                Locale::Ru => format!("Похоже на {track}"),
                Locale::En => format!("Similar to {track}"),
            }),
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
