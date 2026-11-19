//! Player screen, queue origin and playback control strings.

use std::borrow::Cow;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use super::home_recs::MixTitle;
use super::{Locale, home_recs_in, locale, nav_in};
use crate::streaming::Service;

/// Static strings for the player, queue, sound controls and sleep timer.
pub struct PlayerStrings {
    pub now_playing: &'static str,
    pub collapse_tooltip: &'static str,
    pub queue_tooltip: &'static str,
    pub lyrics_tooltip: &'static str,
    pub clear: &'static str,
    pub save_as_playlist: &'static str,
    pub fact_stream: &'static str,
    pub queued_next: &'static str,
    pub added_to_queue: &'static str,
    pub queue_cleared: &'static str,
    pub nothing_playing: &'static str,
    pub wave_direction: &'static str,
    pub shuffle_on_tip: &'static str,
    pub shuffle_queue_tip: &'static str,
    pub prev_tooltip: &'static str,
    pub next_tooltip: &'static str,
    pub buffering_tip: &'static str,
    pub play_pause_tip: &'static str,
    pub queue_ctrl_tooltip: &'static str,
    pub volume_tip: &'static str,
    pub sound_tip: &'static str,
    pub volume_wheel_tip: &'static str,
    pub dislike_tip: &'static str,
    pub add_playlist_tip: &'static str,
    pub sound_line_hint: &'static str,
    pub repeat_off_tip: &'static str,
    pub repeat_all_tip: &'static str,
    pub repeat_one_tip: &'static str,
    pub speed_tip: &'static str,
    pub play_tip: &'static str,
    pub volume_menu: &'static str,
}

impl PlayerStrings {
    /// Formats a quoted "Playing next" / "Up next" toast.
    pub fn next_up_quoted_in(loc: Locale, title: &str) -> String {
        match loc {
            Locale::Ru => format!("Следующим: «{title}»"),
            Locale::En => format!("Up next: “{title}”"),
        }
    }

    /// Formats a plain "Playing next" / "Up next" toast without quotes.
    pub fn next_up_in(loc: Locale, name: &str) -> String {
        match loc {
            Locale::Ru => format!("Следующим: {name}"),
            Locale::En => format!("Up next: {name}"),
        }
    }
}

pub static RU: PlayerStrings = PlayerStrings {
    now_playing: "Сейчас играет",
    collapse_tooltip: "Свернуть (Esc)",
    queue_tooltip: "Очередь (Q)",
    lyrics_tooltip: "Текст песни (L)",
    clear: "Очистить",
    save_as_playlist: "Сохранить как плейлист",
    fact_stream: "Поток",
    queued_next: "Сыграет следующим",
    added_to_queue: "Добавлено в очередь",
    queue_cleared: "Очередь очищена",
    nothing_playing: "Ничего не играет",
    wave_direction: "Сменить направление волны",
    shuffle_on_tip: "Очередь перемешана — нажми, чтобы вернуть порядок",
    shuffle_queue_tip: "Перемешать очередь",
    prev_tooltip: "Назад (Ctrl+←)",
    next_tooltip: "Далее (Ctrl+→)",
    buffering_tip: "Загружается… (Space — пауза)",
    play_pause_tip: "Пауза / играть (Space)",
    queue_ctrl_tooltip: "Очередь (Ctrl+Q)",
    volume_tip: "Громкость: наведи или крути колесо",
    sound_tip: "Звук",
    volume_wheel_tip: "Звук (колесо мыши — громкость)",
    dislike_tip: "Не нравится",
    add_playlist_tip: "В плейлист…",
    sound_line_hint: "Откуда идёт звук и в каком качестве",
    repeat_off_tip: "Повтор выключен — нажми, чтобы повторять очередь",
    repeat_all_tip: "Повторяется вся очередь — нажми, чтобы повторять один трек",
    repeat_one_tip: "Повторяется этот трек — нажми, чтобы выключить",
    speed_tip: "Скорость",
    play_tip: "Играть / пауза (Space)",
    volume_menu: "Громкость…",
};

pub static EN: PlayerStrings = PlayerStrings {
    now_playing: "Now playing",
    collapse_tooltip: "Collapse (Esc)",
    queue_tooltip: "Queue (Q)",
    lyrics_tooltip: "Lyrics (L)",
    clear: "Clear",
    save_as_playlist: "Save as playlist",
    fact_stream: "Streaming",
    queued_next: "Playing next",
    added_to_queue: "Added to queue",
    queue_cleared: "Queue cleared",
    nothing_playing: "Nothing playing",
    wave_direction: "Change Wave direction",
    shuffle_on_tip: "Queue shuffled — select to restore the order.",
    shuffle_queue_tip: "Shuffle the queue",
    prev_tooltip: "Previous (Ctrl+←)",
    next_tooltip: "Next (Ctrl+→)",
    buffering_tip: "Loading… (Space to pause)",
    play_pause_tip: "Pause / play (Space)",
    queue_ctrl_tooltip: "Queue (Ctrl+Q)",
    volume_tip: "Volume: hover or use the scroll wheel.",
    sound_tip: "Sound",
    volume_wheel_tip: "Sound (mouse wheel — volume)",
    dislike_tip: "Don't recommend",
    add_playlist_tip: "Add to playlist…",
    sound_line_hint: "The audio source and quality",
    repeat_off_tip: "Repeat off — select to repeat the queue.",
    repeat_all_tip: "Repeating the queue — select to repeat one track.",
    repeat_one_tip: "Repeating this track — select to turn repeat off.",
    speed_tip: "Speed",
    play_tip: "Play / pause (Space)",
    volume_menu: "Volume…",
};

locale_table! {
    /// Returns player strings for `loc`.
    player_in -> PlayerStrings,
    /// Returns player strings for the active process locale.
    player,
    { Ru => RU, En => EN }
}

/// Static labels for track info tags and audio quality/source facts.
pub struct SoundFactStrings {
    pub tag_title: &'static str,
    pub tag_artist: &'static str,
    pub tag_album: &'static str,
    pub tag_number: &'static str,
    pub local_file: &'static str,
    pub codec: &'static str,
    pub container: &'static str,
    pub bitrate: &'static str,
    pub sample_rate: &'static str,
    pub bit_depth: &'static str,
    pub channels: &'static str,
    pub duration: &'static str,
    pub size: &'static str,
    pub source: &'static str,
    pub khz: &'static str,
    pub bit_unit: &'static str,
    pub mono: &'static str,
    pub stereo: &'static str,
    pub src_torrent: &'static str,
    pub src_stream_saved: &'static str,
    pub src_downloaded: &'static str,
    pub src_spotify: &'static str,
    pub src_youtube: &'static str,
    pub src_soundcloud: &'static str,
    pub original_suffix: &'static str,
}

pub static SOUND_FACTS_RU: SoundFactStrings = SoundFactStrings {
    tag_title: "Название",
    tag_artist: "Исполнитель",
    tag_album: "Альбом",
    tag_number: "Номер",
    local_file: "Локальный файл",
    codec: "Кодек",
    container: "Контейнер",
    bitrate: "Битрейт",
    sample_rate: "Частота",
    bit_depth: "Разрядность",
    channels: "Каналы",
    duration: "Длительность",
    size: "Размер",
    source: "Откуда",
    khz: "кГц",
    bit_unit: "бит",
    mono: "моно",
    stereo: "стерео",
    src_torrent: "Торрент",
    src_stream_saved: "Сохранён со стрима",
    src_downloaded: "Скачан или добавлен файлом",
    src_spotify: "Поток Spotify",
    src_youtube: "Поток YouTube Music",
    src_soundcloud: "Поток SoundCloud",
    original_suffix: "у исходника",
};

pub static SOUND_FACTS_EN: SoundFactStrings = SoundFactStrings {
    tag_title: "Title",
    tag_artist: "Artist",
    tag_album: "Album",
    tag_number: "Track",
    local_file: "Local file",
    codec: "Codec",
    container: "Container",
    bitrate: "Bitrate",
    sample_rate: "Sample rate",
    bit_depth: "Bit depth",
    channels: "Channels",
    duration: "Duration",
    size: "Size",
    source: "Source",
    khz: "kHz",
    bit_unit: "bit",
    mono: "mono",
    stereo: "stereo",
    src_torrent: "Torrent",
    src_stream_saved: "Saved from streaming",
    src_downloaded: "Downloaded or added from a file",
    src_spotify: "Spotify stream",
    src_youtube: "YouTube Music stream",
    src_soundcloud: "SoundCloud stream",
    original_suffix: "from the source",
};

locale_table! {
    /// Returns sound-fact strings for `loc`.
    sound_facts_in -> SoundFactStrings,
    /// Returns sound-fact strings for the active process locale.
    sound_facts,
    { Ru => SOUND_FACTS_RU, En => SOUND_FACTS_EN }
}

/// Formats a sample rate in Hz (`"44,1 кГц"` / `"44.1 kHz"`).
pub fn fmt_khz_in(loc: Locale, hz: u32) -> String {
    let unit = sound_facts_in(loc).khz;
    if hz % 1000 == 0 {
        return format!("{} {unit}", hz / 1000);
    }
    let val = format!("{:.1} {unit}", f64::from(hz) / 1000.0);
    match loc {
        Locale::Ru => val.replace('.', ","),
        Locale::En => val,
    }
}

/// Formats a byte count in MiB (`"25,5 МБ"` / `"25.5 MB"`).
pub fn fmt_mb_in(loc: Locale, bytes: u64) -> String {
    let unit = super::common_in(loc).mb;
    let val = format!("{:.1} {unit}", bytes as f64 / 1_048_576.0);
    match loc {
        Locale::Ru => val.replace('.', ","),
        Locale::En => val,
    }
}

/// Formats the speed mark's accessibility label (`"Скорость: ×1.2"` / `"Speed: ×1.2"`).
pub fn fmt_speed_mark_in(loc: Locale, text: &str) -> String {
    match loc {
        Locale::Ru => format!("Скорость: {text}"),
        Locale::En => format!("Speed: {text}"),
    }
}

/// Formats the speed mark's accessibility label in the active locale.
pub fn fmt_speed_mark(text: &str) -> String {
    fmt_speed_mark_in(locale(), text)
}

/// Typed title for a running radio or autoplay queue, replacing raw Russian
/// title parsing across runtime and UI screens.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RadioTitle {
    #[default]
    Wave,
    Discover,
    Seed(String),
    Album(String),
    Playlist { name: String, liked: bool },
    Autoplay(String),
    Mix(MixTitle),
}

impl RadioTitle {
    /// Localized standalone radio title in `loc`.
    pub fn title_in(&self, loc: Locale) -> Cow<'_, str> {
        match self {
            Self::Wave => Cow::Borrowed(home_recs_in(loc).my_wave),
            Self::Discover => Cow::Borrowed(home_recs_in(loc).discover),
            Self::Seed(s) => Cow::Owned(match loc {
                Locale::Ru => format!("Радио · {s}"),
                Locale::En => format!("Radio · {s}"),
            }),
            Self::Album(s) => Cow::Owned(match loc {
                Locale::Ru => format!("Радио по альбому «{s}»"),
                Locale::En => format!("Album radio “{s}”"),
            }),
            Self::Playlist { name, liked } => {
                let target = if *liked { nav_in(loc).liked } else { name.as_str() };
                Cow::Owned(match loc {
                    Locale::Ru => format!("Радио по плейлисту «{target}»"),
                    Locale::En => format!("Playlist radio “{target}”"),
                })
            }
            Self::Autoplay(s) => Cow::Owned(match loc {
                Locale::Ru => format!("Похожее на «{s}»"),
                Locale::En => format!("Similar to “{s}”"),
            }),
            Self::Mix(mix) => mix.title_in(loc),
        }
    }

    /// Localized standalone radio title in the active process locale.
    pub fn title(&self) -> Cow<'_, str> {
        self.title_in(locale())
    }

    /// Localized queue header for `loc` (`"Микс · …"` / `"Mix · …"` for mixes,
    /// wave, discover and autoplay; standalone title for explicit radios).
    pub fn queue_header_in(&self, loc: Locale) -> String {
        match self {
            Self::Seed(_) | Self::Album(_) | Self::Playlist { .. } => self.title_in(loc).into_owned(),
            Self::Wave | Self::Discover | Self::Autoplay(_) | Self::Mix(_) => {
                let title = self.title_in(loc);
                match loc {
                    Locale::Ru => format!("Микс · {title}"),
                    Locale::En => format!("Mix · {title}"),
                }
            }
        }
    }

    /// Localized queue header in the active process locale.
    pub fn queue_header(&self) -> String {
        self.queue_header_in(locale())
    }

    /// Seed name when this radio grows from a track, artist, stream, autoplay
    /// seed, or album.
    pub fn seed_name(&self) -> Option<&str> {
        match self {
            Self::Seed(s) | Self::Autoplay(s) | Self::Album(s) => Some(s.as_str()),
            Self::Wave | Self::Discover | Self::Playlist { .. } | Self::Mix(_) => None,
        }
    }

    /// Converts this radio title into a typed [`QueueOrigin`].
    pub fn to_queue_origin(&self) -> QueueOrigin {
        match self {
            Self::Wave => QueueOrigin::Wave,
            Self::Discover => QueueOrigin::Discover,
            Self::Seed(s) => QueueOrigin::Radio(RadioOrigin::Named(s.clone())),
            Self::Album(s) => QueueOrigin::Radio(RadioOrigin::Album(s.clone())),
            Self::Playlist { liked: true, .. } => QueueOrigin::Radio(RadioOrigin::Liked),
            Self::Playlist { name, liked: false } => QueueOrigin::Radio(RadioOrigin::Playlist(name.clone())),
            Self::Autoplay(s) => QueueOrigin::Mix(MixTitle::BecauseYouLiked(s.clone())),
            Self::Mix(mix) => QueueOrigin::Mix(mix.clone()),
        }
    }

    /// Parses a legacy Russian radio title into a typed [`RadioTitle`].
    pub fn from_legacy(raw: &str) -> Self {
        let s = raw.trim();
        match s {
            "Моя волна" => return Self::Wave,
            "Новое для тебя" => return Self::Discover,
            _ => {}
        }
        if let Some(rest) = s.strip_prefix("Радио · ") {
            return Self::Seed(rest.to_owned());
        }
        if let Some(rest) = strip_guillemets(s, "Радио по альбому ") {
            return Self::Album(rest.to_owned());
        }
        if let Some(rest) = strip_guillemets(s, "Радио по плейлисту ") {
            let liked = rest == "Любимые";
            return Self::Playlist {
                name: if liked { String::new() } else { rest.to_owned() },
                liked,
            };
        }
        if let Some(rest) = strip_guillemets(s, "Похожее на ") {
            return Self::Autoplay(rest.to_owned());
        }
        Self::Mix(MixTitle::from_legacy(s))
    }
}

/// Radio seed description inside [`QueueOrigin::Radio`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RadioOrigin {
    Named(String),
    Album(String),
    Playlist(String),
    Liked,
}

/// Typed origin of the active queue, replacing raw Russian context strings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueOrigin {
    #[default]
    None,
    Album(String),
    Playlist(String),
    Liked,
    Artist(String),
    Mix(MixTitle),
    Wave,
    Radio(RadioOrigin),
    Search { service: Option<Service> },
    Library,
    Downloaded,
    ListenAgain,
    Discover,
    World(String),
    Wishlist,
    /// A preview played from the «Найти песню…» dialog.
    FindSong,
    Custom(String),
}

impl QueueOrigin {
    /// Formats the Now Playing header line and the exact byte range of the
    /// clickable entity name when `has_link` is true.
    pub fn header_in(&self, loc: Locale, has_link: bool) -> (String, Option<Range<usize>>) {
        let quoted = |prefix: &str, name: &str| -> (String, Option<Range<usize>>) {
            let (open, close) = match loc {
                Locale::Ru => ('«', '»'),
                Locale::En => ('“', '”'),
            };
            let mut out = String::with_capacity(prefix.len() + name.len() + open.len_utf8() + close.len_utf8());
            out.push_str(prefix);
            out.push(open);
            let start = out.len();
            out.push_str(name);
            let end = out.len();
            out.push(close);
            let link = (has_link && start < end).then_some(start..end);
            (out, link)
        };

        match self {
            Self::None => (player_in(loc).now_playing.to_owned(), None),
            Self::Album(name) => quoted(
                match loc {
                    Locale::Ru => "Играет из альбома ",
                    Locale::En => "Playing from album ",
                },
                name,
            ),
            Self::Playlist(name) => quoted(
                match loc {
                    Locale::Ru => "Играет из плейлиста ",
                    Locale::En => "Playing from playlist ",
                },
                name,
            ),
            Self::Liked => quoted(
                match loc {
                    Locale::Ru => "Играет из плейлиста ",
                    Locale::En => "Playing from playlist ",
                },
                nav_in(loc).liked,
            ),
            Self::Artist(name) => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                name,
            ),
            Self::Mix(mix) => {
                let title = mix.title_in(loc);
                quoted(
                    match loc {
                        Locale::Ru => "Играет из микса ",
                        Locale::En => "Playing from mix ",
                    },
                    &title,
                )
            }
            Self::Wave => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                home_recs_in(loc).my_wave,
            ),
            Self::Radio(radio) => match radio {
                RadioOrigin::Named(name) => quoted(
                    match loc {
                        Locale::Ru => "Играет из радио ",
                        Locale::En => "Playing from radio ",
                    },
                    name,
                ),
                RadioOrigin::Album(name) => quoted(
                    match loc {
                        Locale::Ru => "Играет из радио по альбому ",
                        Locale::En => "Playing from album radio ",
                    },
                    name,
                ),
                RadioOrigin::Playlist(name) => quoted(
                    match loc {
                        Locale::Ru => "Играет из радио по плейлисту ",
                        Locale::En => "Playing from playlist radio ",
                    },
                    name,
                ),
                RadioOrigin::Liked => quoted(
                    match loc {
                        Locale::Ru => "Играет из радио по плейлисту ",
                        Locale::En => "Playing from playlist radio ",
                    },
                    nav_in(loc).liked,
                ),
            },
            Self::Search { service: None } => (
                match loc {
                    Locale::Ru => "Играет из поиска".to_owned(),
                    Locale::En => "Playing from search".to_owned(),
                },
                None,
            ),
            Self::Search { service: Some(svc) } => (
                match loc {
                    Locale::Ru => format!("Играет из поиска {}", svc.display_name()),
                    Locale::En => format!("Playing from {} search", svc.display_name()),
                },
                None,
            ),
            Self::Library => (
                match loc {
                    Locale::Ru => "Играет из библиотеки".to_owned(),
                    Locale::En => "Playing from your library".to_owned(),
                },
                None,
            ),
            Self::Downloaded => (
                match loc {
                    Locale::Ru => "Играет из скачанного".to_owned(),
                    Locale::En => "Playing from downloads".to_owned(),
                },
                None,
            ),
            Self::ListenAgain => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                home_recs_in(loc).listen_again,
            ),
            Self::Discover => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                home_recs_in(loc).discover,
            ),
            Self::World(title) => quoted(
                match loc {
                    Locale::Ru => "Играет из мира ",
                    Locale::En => "Playing from world ",
                },
                title,
            ),
            Self::Wishlist => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                match loc {
                    Locale::Ru => "Хочу скачать",
                    Locale::En => "Wishlist",
                },
            ),
            Self::FindSong => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                match loc {
                    Locale::Ru => "Найти песню",
                    Locale::En => "Find a song",
                },
            ),
            Self::Custom(s) => quoted(
                match loc {
                    Locale::Ru => "Играет из ",
                    Locale::En => "Playing from ",
                },
                s,
            ),
        }
    }

    /// Default playlist name suggested when saving the queue as a playlist.
    pub fn save_name_in(&self, loc: Locale) -> Option<String> {
        match self {
            Self::None => None,
            Self::Album(s)
            | Self::Playlist(s)
            | Self::Artist(s)
            | Self::World(s)
            | Self::Custom(s) => Some(s.clone()),
            Self::Liked => Some(nav_in(loc).liked.to_owned()),
            Self::Mix(mix) => Some(mix.title_in(loc).into_owned()),
            Self::Wave => Some(home_recs_in(loc).my_wave.to_owned()),
            Self::Radio(r) => Some(match r {
                RadioOrigin::Named(s) | RadioOrigin::Album(s) | RadioOrigin::Playlist(s) => s.clone(),
                RadioOrigin::Liked => nav_in(loc).liked.to_owned(),
            }),
            Self::Search { service: None } => Some(match loc {
                Locale::Ru => "Поиск".to_owned(),
                Locale::En => "Search".to_owned(),
            }),
            Self::Search { service: Some(svc) } => Some(match loc {
                Locale::Ru => format!("Поиск {}", svc.display_name()),
                Locale::En => format!("{} search", svc.display_name()),
            }),
            Self::Library => Some(nav_in(loc).library.to_owned()),
            Self::Downloaded => Some(match loc {
                Locale::Ru => "Скачанное".to_owned(),
                Locale::En => "Downloads".to_owned(),
            }),
            Self::ListenAgain => Some(home_recs_in(loc).listen_again.to_owned()),
            Self::Discover => Some(home_recs_in(loc).discover.to_owned()),
            Self::Wishlist => Some(match loc {
                Locale::Ru => "Хочу скачать".to_owned(),
                Locale::En => "Wishlist".to_owned(),
            }),
            Self::FindSong => Some(match loc {
                Locale::Ru => "Найти песню".to_owned(),
                Locale::En => "Find a song".to_owned(),
            }),
        }
    }

    /// Default playlist name in the active locale.
    pub fn save_name(&self) -> Option<String> {
        self.save_name_in(locale())
    }

    /// Legacy Russian string representation for backward compatibility with
    /// older `session.json` and Connect peers.
    pub fn legacy_text(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Album(s) => Some(format!("альбома «{s}»")),
            Self::Playlist(s) => Some(format!("плейлиста «{s}»")),
            Self::Liked => Some("плейлиста «Любимые»".to_owned()),
            Self::Artist(s) => Some(format!("исполнителя «{s}»")),
            Self::Mix(mix) => Some(format!("микса «{}»", mix.title_in(Locale::Ru))),
            Self::Wave => Some("микса «Моя волна»".to_owned()),
            Self::Radio(RadioOrigin::Named(s)) => Some(format!("радио «{s}»")),
            Self::Radio(RadioOrigin::Album(s)) => Some(format!("радио по альбому «{s}»")),
            Self::Radio(RadioOrigin::Playlist(s)) => Some(format!("радио по плейлисту «{s}»")),
            Self::Radio(RadioOrigin::Liked) => Some("радио по плейлисту «Любимые»".to_owned()),
            Self::Search { service: None } => Some("поиска".to_owned()),
            Self::Search { service: Some(svc) } => Some(format!("поиска {}", svc.display_name())),
            Self::Library => Some("библиотеки".to_owned()),
            Self::Downloaded => Some("скачанного".to_owned()),
            Self::ListenAgain => Some("Слушать снова".to_owned()),
            Self::Discover => Some("Новое для тебя".to_owned()),
            Self::World(s) => Some(format!("мира «{s}»")),
            Self::Wishlist => Some("«Хочу скачать»".to_owned()),
            Self::FindSong => Some("Найти песню".to_owned()),
            Self::Custom(s) => Some(s.clone()),
        }
    }

    /// Parses a legacy context string (from older `session.json`, Connect
    /// peers or stringly-typed commands) into a typed [`QueueOrigin`].
    pub fn from_legacy(raw: Option<&str>) -> Self {
        let Some(c) = raw.map(str::trim).filter(|c| !c.is_empty()) else {
            return Self::None;
        };
        if let Some(rest) = c.strip_prefix("поиска") {
            let service_name = rest.split(':').next().unwrap_or_default().trim();
            return Self::Search { service: parse_service_display(service_name) };
        }
        if c.starts_with("Поиск ") {
            return Self::Search { service: None };
        }
        if let Some((svc_name, q)) = c.split_once(" · ")
            && q.trim_start().starts_with('«')
        {
            return Self::Search { service: parse_service_display(svc_name.trim()) };
        }
        match c {
            "библиотеки" => return Self::Library,
            "скачанного" => return Self::Downloaded,
            "Слушать снова" => return Self::ListenAgain,
            "Новое для тебя" => return Self::Discover,
            "Любимые" | "плейлиста «Любимые»" => return Self::Liked,
            "«Хочу скачать»" | "Хочу скачать" => return Self::Wishlist,
            "Найти песню" | "Find song" => return Self::FindSong,
            "микса «Моя волна»" | "Моя волна" => return Self::Wave,
            _ => {}
        }
        if let Some(name) = strip_guillemets(c, "альбома ") {
            return Self::Album(name.to_owned());
        }
        if let Some(name) = strip_guillemets(c, "плейлиста ") {
            return if name == "Любимые" { Self::Liked } else { Self::Playlist(name.to_owned()) };
        }
        if let Some(name) = strip_guillemets(c, "микса ") {
            return Self::Mix(MixTitle::from_legacy(name));
        }
        if let Some(name) = strip_guillemets(c, "радио ") {
            return Self::Radio(RadioOrigin::Named(name.to_owned()));
        }
        if let Some(name) = strip_guillemets(c, "радио по альбому ") {
            return Self::Radio(RadioOrigin::Album(name.to_owned()));
        }
        if let Some(name) = strip_guillemets(c, "радио по плейлисту ") {
            return if name == "Любимые" {
                Self::Radio(RadioOrigin::Liked)
            } else {
                Self::Radio(RadioOrigin::Playlist(name.to_owned()))
            };
        }
        if let Some(name) = strip_guillemets(c, "мира ") {
            return Self::World(name.to_owned());
        }
        if let Some(name) = strip_guillemets(c, "исполнителя ") {
            return Self::Artist(name.to_owned());
        }
        if let Some(inner) = strip_guillemets(c, "") {
            return Self::Custom(inner.to_owned());
        }
        Self::Custom(c.to_owned())
    }
}

impl From<&str> for QueueOrigin {
    fn from(s: &str) -> Self {
        Self::from_legacy(Some(s))
    }
}

impl From<String> for QueueOrigin {
    fn from(s: String) -> Self {
        Self::from_legacy(Some(&s))
    }
}

fn strip_guillemets<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(prefix)?;
    rest.strip_prefix('«')?.strip_suffix('»')
}

fn parse_service_display(name: &str) -> Option<Service> {
    if name.is_empty() {
        return None;
    }
    Service::ALL.iter().copied().find(|s| s.display_name().eq_ignore_ascii_case(name) || s.as_str().eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_mark_label_translates() {
        assert_eq!(fmt_speed_mark_in(Locale::Ru, "×1.2"), "Скорость: ×1.2");
        assert_eq!(fmt_speed_mark_in(Locale::En, "×1.2"), "Speed: ×1.2");
        assert_eq!(player_in(Locale::Ru).nothing_playing, "Ничего не играет");
        assert_eq!(player_in(Locale::En).nothing_playing, "Nothing playing");
        assert_eq!(player_in(Locale::Ru).repeat_one_tip, "Повторяется этот трек — нажми, чтобы выключить");
        assert_eq!(player_in(Locale::En).repeat_one_tip, "Repeating this track — select to turn repeat off.");
    }

    #[test]
    fn find_song_origin_translates_and_parses_legacy() {
        let o = QueueOrigin::FindSong;
        assert_eq!(o.header_in(Locale::Ru, false).0, "Играет из «Найти песню»");
        assert_eq!(o.header_in(Locale::En, false).0, "Playing from “Find a song”");
        assert_eq!(o.save_name_in(Locale::Ru).as_deref(), Some("Найти песню"));
        assert_eq!(o.save_name_in(Locale::En).as_deref(), Some("Find a song"));
        assert_eq!(o.legacy_text().as_deref(), Some("Найти песню"));
        assert_eq!(QueueOrigin::from_legacy(Some("Найти песню")), o);
        assert_eq!(QueueOrigin::from_legacy(Some("Find song")), o);
    }
}
