//! What every settings row is: an id other screens can link to ("настроить"
//! opens Settings on that row, scrolls to it and makes it glow), a one-line
//! explanation and, for the looks, a small live preview painted by the real
//! painters.
//!
//! The explanation shows in a card under the row while it is hovered (over
//! one option of a choice: that option, previewed). The small ⓘ button next
//! to the label pins a larger card open: the explanation and, for a choice,
//! every option side by side with its own preview; a click on one picks it.
//! The pinned card closes on the button, a click outside or Esc.
//!
//! A new row opts in by getting a [`SettingId`] variant, a [`SettingId::tab`]
//! and a [`SettingId::help`] line (a choice also [`SettingId::choices`]);
//! [`super::settings`] does the rest.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    Align, Area, Color32, CursorIcon, Galley, Id, Key, Modifiers, Order, Painter, Pos2, Rect, Sense, Shape, Stroke,
    Ui, Vec2, pos2, text::LayoutJob, vec2,
};
use nikgob_view::{self as v, Prefs};

use crate::screens::{bg_shapes, lyrics, now_playing};
use crate::{
    Animated, ColorScheme, Contrast, Elevation, Icon, SchemeVariant, Theme, TypeRole,
    components::{anim, paint, paint::Radii, slider::wave_line},
    tokens::shape,
};

/// One settings row (or block of fields).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingId {
    // Внешний вид.
    Design,
    Theme,
    Contrast,
    Scheme,
    ProgressWave,
    NpVolume,
    CoverShape,
    ShapeRange,
    ShapePick,
    CoverSpin,
    CoverDir,
    CoverPulse,
    Ring,
    RingStyle,
    RingSpin,
    RingDir,
    RingBeat,
    Ambient,
    AmbientStrength,
    BgShapes,
    BgCount,
    BgSize,
    BgOpacity,
    BgSpeed,
    BgReact,
    BgColors,
    BgHue,
    LyricsStyle,
    WaveFallback,
    WaveJump,
    LyricsDim,
    LyricsScale,
    LyricsCascade,
    LyricsGlow,
    LyricsGap,
    LyricsGapWaves,
    LyricsCountdown,
    RailCompact,
    NpMotion,
    ReducedMotion,
    FpsLimit,
    // Основные.
    CoverClickPause,
    DblClickShape,
    Keys,
    SaveLyrics,
    LyricsReturn,
    OutputDevice,
    Gapless,
    Crossfade,
    Normalization,
    LibraryDir,
    MastersDir,
    Inbox,
    NamingRules,
    Trash,
    ExportData,
    OpusBitrate,
    FakeFlac,
    MusicBrainz,
    AcoustId,
    Prowlarr,
    NoSeed,
    SeedLimit,
    SeedDays,
    SeedRatio,
    DownloadLimit,
    UploadLimit,
    Spotify,
    YouTube,
    Nodes,
}

/// Settings tabs, in the order of the tab switch.
pub const TAB_GENERAL: usize = 0;
pub const TAB_APPEARANCE: usize = 1;

/// The live sample a card draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Preview {
    Palette,
    Ambient,
    Wave,
    Cover,
    Ring,
    Background,
    Lyrics,
    Gap,
    Motion,
    Volume,
}

/// One option of a choice row: its label (as on the button) and what it does.
#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub label: &'static str,
    pub help: &'static str,
}

/// A [`Choice`] (a literal, so the lists below are `'static`).
macro_rules! c {
    ($label:expr, $help:expr) => {
        Choice { label: $label, help: $help }
    };
}

const OPUS: [u32; 4] = [128, 160, 192, 256];

impl SettingId {
    /// The tab the row lives on.
    pub fn tab(self) -> usize {
        use SettingId::*;
        match self {
            Design | Theme | Contrast | Scheme | ProgressWave | NpVolume | CoverShape | ShapeRange | ShapePick
            | CoverSpin | CoverDir | CoverPulse | Ring | RingStyle | RingSpin | RingDir | RingBeat | Ambient
            | AmbientStrength | BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue
            | LyricsStyle | WaveFallback | WaveJump | LyricsDim | LyricsScale | LyricsCascade | LyricsGlow
            | LyricsGap | LyricsGapWaves | LyricsCountdown | RailCompact | NpMotion | ReducedMotion | FpsLimit => {
                TAB_APPEARANCE
            }
            _ => TAB_GENERAL,
        }
    }

    /// One line on what the option changes.
    pub fn help(self) -> &'static str {
        use SettingId::*;
        match self {
            Design => "Общий стиль интерфейса. Пока есть только Material 3 Expressive.",
            Theme => "Светлая, тёмная или как в системе.",
            Contrast => "Насколько текст и элементы отличаются от фона.",
            Scheme => "Как из обложки трека получается палитра: от спокойной до яркой.",
            ProgressWave => "Сыгранная часть полосы прогресса бежит волной, пока играет музыка.",
            NpVolume => "Как в открытом треке показывается громкость: ползунок всегда, плашка при изменении или только кнопка звука.",
            CoverShape => "Квадратная обложка, одна фигура Material или случайная на каждый трек.",
            ShapeRange => "Набор фигур, из которых выбирается случайная. Клик — добавить или убрать.",
            ShapePick => "Какой фигурой обрезана обложка, пока играет музыка.",
            CoverSpin => "Контур обложки медленно крутится, сама картинка стоит.",
            CoverDir => "В какую сторону крутится контур обложки.",
            CoverPulse => "Обложка чуть увеличивается на каждой доле.",
            Ring => "Точки или бары вокруг обложки, которые двигаются под музыку.",
            RingStyle => "Круг из точек, точки по контуру фигуры или бары-спектр.",
            RingSpin => "Кольцо медленно вращается, пока играет музыка.",
            RingDir => "В какую сторону вращается кольцо.",
            RingBeat => "Точки отскакивают на ударных и там, где громче спектр.",
            Ambient => "Мягкие цветные облака за плеером; «Под звук» — дышат с басом.",
            AmbientStrength => "Насколько заметны облака: от лёгкой дымки до насыщенного цвета.",
            BgShapes => "Фигуры Material за открытым треком: прыгают, парят или меняют форму.",
            BgCount => "Сколько фигур на фоне.",
            BgSize => "Размер фигур на фоне.",
            BgOpacity => "Насколько фигуры заметны на фоне.",
            BgSpeed => "Как быстро фигуры плывут, вращаются и прыгают.",
            BgReact => "Насколько сильно фигуры отзываются на музыку.",
            BgColors => "Цвета фигур: контейнеры темы, один акцент, все яркие акценты или свой оттенок.",
            BgHue => "Оттенок фигур в режиме «Свой». Светлота подстраивается под тему, поэтому цвет не спорит с интерфейсом.",
            LyricsStyle => "Как подсвечивается строка, которую сейчас поют.",
            WaveFallback => "Что показывать вместо «Волны», если у текста нет разметки по слогам.",
            WaveJump => "Как высоко подпрыгивают буквы в анимации «Волна».",
            LyricsDim => "Насколько приглушены строки, которые сейчас не поют.",
            LyricsScale => "Размер шрифта в тексте песен.",
            LyricsCascade => "Дальние строки догоняют прокрутку с задержкой, как каскад.",
            LyricsGlow => "Спетая часть строки слегка светится.",
            LyricsGap => "Как выглядит индикатор паузы между строками.",
            LyricsGapWaves => "Сколько гребней у волнистого индикатора паузы.",
            LyricsCountdown => "В последние секунды паузы индикатор считает 3-2-1.",
            RailCompact => "Левая панель остаётся узкой — только значки разделов, подписи и кнопка раскрытия не показываются.",
            NpMotion => "Скорость и характер анимации, с которой открывается экран трека.",
            ReducedMotion => "Убирает отскоки, вращение и волны во всём интерфейсе.",
            FpsLimit => "Максимум кадров в секунду. Меньше — тише вентилятор и дольше батарея.",
            CoverClickPause => "Клик по обложке в открытом треке ставит на паузу и снимает с неё.",
            DblClickShape => "Двойной клик в открытом треке меняет фигуру обложки.",
            Keys => "Горячие клавиши в открытом треке.",
            SaveLyrics => "Найденный в сети текст записывается в теги файла и работает без интернета.",
            LyricsReturn => "Через сколько секунд после ручной прокрутки текст вернётся к текущей строке.",
            OutputDevice => "Куда идёт звук. Переключается без паузы.",
            Gapless => "Треки альбома играют встык, без тишины между ними.",
            Crossfade => "Конец трека плавно перетекает в начало следующего.",
            Normalization => "Выравнивает громкость треков по ReplayGain / R128.",
            LibraryDir => "Где лежит музыка библиотеки.",
            MastersDir => "Куда складываются исходные FLAC после перекодирования в Opus.",
            Inbox => "Папка, из которой новые файлы импортируются в библиотеку.",
            NamingRules => "Как называются папки и файлы после импорта.",
            Trash => "Включена — удалённое 30 дней лежит в Библиотека → Корзина; выключена — удаление сразу стирает файлы.",
            ExportData => "Лайки и плейлисты лежат файлами рядом с музыкой.",
            OpusBitrate => "Качество Opus при импорте: выше — лучше звук и больше файлы.",
            FakeFlac => "Ищет FLAC, перекодированные из MP3: у них обрезан спектр.",
            MusicBrainz => "Контакт, который MusicBrainz просит указывать в запросах.",
            AcoustId => "Ключ для опознания треков по звуку.",
            Prowlarr => "Адрес и ключ Prowlarr — через него идёт поиск раздач.",
            NoSeed => "Скачанное сразу перестаёт раздаваться.",
            SeedLimit => "Сколько всего можно отдать, прежде чем раздачи остановятся.",
            SeedDays => "Сколько дней держать раздачу, потом чистка.",
            SeedRatio => "Раздача останавливается, когда отдано в столько раз больше скачанного.",
            DownloadLimit => "Ограничение скорости загрузки торрентов.",
            UploadLimit => "Ограничение скорости отдачи торрентов.",
            Spotify => "Импорт лайков и плейлистов из Spotify — появится позже.",
            YouTube => "Импорт из YouTube Music — появится позже.",
            Nodes => "Синхронизация и бэкап на другие устройства — появится позже.",
        }
    }

    /// The options of a choice row (segmented buttons or a menu), in order;
    /// empty for every other row.
    pub fn choices(self) -> &'static [Choice] {
        match self {
            Self::Theme => &[
                c!("Светлая", "Светлый фон и тёмный текст — лучше днём."),
                c!("Тёмная", "Тёмный фон — спокойнее вечером, обложки ярче."),
                c!("Системная", "Как в Windows: переключается вместе с системой."),
            ],
            Self::Contrast => &[
                c!("Обычный", "Стандартные цвета Material 3."),
                c!("Средний", "Текст и границы заметнее на фоне."),
                c!("Высокий", "Самый чёткий текст, цвета жёстче."),
            ],
            Self::Scheme => &[
                c!("Content", "Ближе всего к цветам обложки."),
                c!("Tonal Spot", "Спокойная приглушённая палитра, как в Android."),
                c!("Vibrant", "Насыщенные яркие акценты."),
                c!("Expressive", "Смелые сдвиги оттенков, неожиданные сочетания."),
                c!("Fidelity", "Как Content, но контейнеры точно в цвет обложки."),
                c!("Monochrome", "Только оттенки серого."),
                c!("Neutral", "Почти серая, с лёгким тоном обложки."),
            ],
            Self::NpVolume => &[
                c!("Всегда", "Ползунок всегда рядом с кнопкой звука."),
                c!("При изменении", "Внизу по центру всплывает плашка, пока меняешь громкость, и прячется через пару секунд."),
                c!("Нет", "Только кнопка звука; громкость — колесом мыши."),
            ],
            Self::CoverShape => &[
                c!("Квадрат", "Обложка как есть, без обрезки."),
                c!("Фигура", "Обложка обрезана одной выбранной фигурой."),
                c!("Случайная", "Новая фигура на каждом треке."),
            ],
            Self::CoverDir | Self::RingDir => &[
                c!("По часовой", "Крутится по часовой стрелке."),
                c!("Против", "Крутится против часовой стрелки."),
            ],
            Self::RingStyle => &[
                c!("Круг", "Точки мягким волнистым кругом, какой бы ни была форма."),
                c!("Контур", "Точки идут вдоль контура обложки на равном расстоянии."),
                c!("Бары", "Точки вырастают в столбики спектра."),
            ],
            Self::Ambient => &[
                c!("Выкл", "Ровный фон, без облаков."),
                c!("Свечение", "Облака медленно плывут и не слушают музыку."),
                c!("Под звук", "Облака дышат и разгораются с басом."),
            ],
            Self::BgShapes => &[
                c!("Выкл", "Фон без фигур."),
                c!("Прыгают", "Лежат внизу и подпрыгивают на сильных долях."),
                c!("Парят", "Плавают по экрану и светятся в такт."),
                c!("Меняют форму", "Плывут и превращаются в другие фигуры на ударах."),
            ],
            Self::BgColors => &[
                c!("Тема", "Мягкие контейнеры темы — спокойно и ненавязчиво."),
                c!("Акцент", "Один основной цвет темы."),
                c!("Яркие", "Основной, вторичный и третичный акценты."),
                c!("Свой", "Оттенок, выбранный в строке «Свой цвет»."),
            ],
            Self::LyricsStyle => &[
                c!("Караоке", "Строка заливается цветом по слогам."),
                c!("Фокус", "Текущая строка подсвечивается целиком."),
                c!("Волна", "Буквы загораются и подпрыгивают одна за другой."),
            ],
            Self::WaveFallback => &[
                c!("Волна — прыгающие буквы", "Буквы прыгают и без разметки, по примерному времени."),
                c!("Спокойный Фокус", "Без разметки строка просто подсвечивается целиком."),
            ],
            Self::LyricsGap => &[
                c!("Линия", "Тонкое ровное кольцо."),
                c!("Волна", "Тонкое волнистое кольцо."),
                c!("Толстая линия", "Толстое ровное кольцо."),
                c!("Толстая волна", "Толстое волнистое кольцо."),
            ],
            Self::NpMotion => &[
                c!("Плавно", "Мягко и неторопливо, без отскока."),
                c!("Живо", "Быстро, с небольшим отскоком в конце."),
                c!("Быстро", "Почти мгновенно, без отскока."),
            ],
            Self::DblClickShape => &[
                c!("Выкл", "Двойной клик ничего не меняет."),
                c!("Только в режиме «Случайная»", "Новая фигура, только когда она выбирается случайно."),
                c!("Всегда", "Всегда; выбранная фигура сменяется следующей."),
            ],
            Self::OpusBitrate => &[
                c!("128", "Компактно; на слух почти как оригинал."),
                c!("160", "Баланс качества и размера."),
                c!("192", "С запасом для хороших наушников."),
                c!("256", "Максимум; файлы вдвое больше, чем при 128."),
            ],
            _ => &[],
        }
    }

    /// Which of [`Self::choices`] `p` has.
    pub fn chosen(self, p: &Prefs) -> usize {
        match self {
            Self::Theme => match p.theme {
                v::ThemeMode::Light => 0,
                v::ThemeMode::Dark => 1,
                v::ThemeMode::System => 2,
            },
            Self::Contrast => usize::from(p.contrast).min(2),
            Self::NpVolume => match (p.np_volume, p.np_volume_pop) {
                (true, _) => 0,
                (false, true) => 1,
                (false, false) => 2,
            },
            Self::Scheme => usize::from(p.scheme_variant).min(6),
            Self::CoverShape => match p.cover_shape {
                v::CoverShape::Original => 0,
                v::CoverShape::Random => 2,
                _ => 1,
            },
            Self::CoverDir => usize::from(p.cover_ccw),
            Self::RingDir => usize::from(p.ring_ccw),
            Self::RingStyle => match p.ring_style {
                v::RingStyle::Circle => 0,
                v::RingStyle::Contour => 1,
                v::RingStyle::Bars => 2,
            },
            Self::Ambient => match p.ambient {
                v::Ambient::Off => 0,
                v::Ambient::Glow => 1,
                v::Ambient::Reactive => 2,
            },
            Self::BgShapes => match p.bg_shapes {
                v::BgShapes::Off => 0,
                v::BgShapes::Bounce => 1,
                v::BgShapes::Float => 2,
                v::BgShapes::Morph => 3,
            },
            Self::BgColors => usize::from(p.bg_shapes_colors).min(3),
            Self::LyricsStyle => usize::from(p.lyrics_style).min(2),
            Self::WaveFallback => usize::from(p.wave_syllables_only),
            Self::LyricsGap => usize::from(p.lyrics_gap_style).min(3),
            Self::NpMotion => usize::from(p.np_motion).min(2),
            Self::DblClickShape => usize::from(p.dbl_click_shape).min(2),
            Self::OpusBitrate => OPUS.iter().position(|b| *b == p.opus_bitrate).unwrap_or(1),
            _ => 0,
        }
    }

    /// Sets option `i` of [`Self::choices`] in `p`.
    pub fn choose(self, p: &mut Prefs, i: usize) {
        match self {
            Self::Theme => p.theme = [v::ThemeMode::Light, v::ThemeMode::Dark, v::ThemeMode::System][i.min(2)],
            Self::Contrast => p.contrast = i.min(2) as u8,
            Self::NpVolume => (p.np_volume, p.np_volume_pop) = [(true, false), (false, true), (false, false)][i.min(2)],
            Self::Scheme => p.scheme_variant = i.min(6) as u8,
            Self::CoverShape => {
                p.cover_shape = match i {
                    0 => v::CoverShape::Original,
                    2 => v::CoverShape::Random,
                    // A fixed shape stays; from the others the first of the pool.
                    _ if !matches!(p.cover_shape, v::CoverShape::Original | v::CoverShape::Random) => p.cover_shape,
                    _ => now_playing::random_pool(&p.random_shapes)[0],
                }
            }
            Self::CoverDir => p.cover_ccw = i == 1,
            Self::RingDir => p.ring_ccw = i == 1,
            Self::RingStyle => p.ring_style = [v::RingStyle::Circle, v::RingStyle::Contour, v::RingStyle::Bars][i.min(2)],
            Self::Ambient => p.ambient = [v::Ambient::Off, v::Ambient::Glow, v::Ambient::Reactive][i.min(2)],
            Self::BgShapes => {
                p.bg_shapes = [v::BgShapes::Off, v::BgShapes::Bounce, v::BgShapes::Float, v::BgShapes::Morph][i.min(3)]
            }
            Self::BgColors => p.bg_shapes_colors = i.min(3) as u8,
            Self::LyricsStyle => p.lyrics_style = i.min(2) as u8,
            Self::WaveFallback => p.wave_syllables_only = i == 1,
            Self::LyricsGap => p.lyrics_gap_style = i.min(3) as u8,
            Self::NpMotion => p.np_motion = i.min(2) as u8,
            Self::DblClickShape => p.dbl_click_shape = i.min(2) as u8,
            Self::OpusBitrate => p.opus_bitrate = OPUS[i.min(OPUS.len() - 1)],
            _ => {}
        }
    }

    fn preview(self) -> Option<Preview> {
        use SettingId::*;
        Some(match self {
            Theme | Contrast | Scheme => Preview::Palette,
            Ambient | AmbientStrength => Preview::Ambient,
            ProgressWave | ReducedMotion => Preview::Wave,
            CoverShape | ShapeRange | ShapePick | CoverSpin | CoverDir | CoverPulse => Preview::Cover,
            Ring | RingStyle | RingSpin | RingDir | RingBeat => Preview::Ring,
            BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue => Preview::Background,
            LyricsStyle | WaveFallback | WaveJump | LyricsDim | LyricsScale | LyricsGlow => Preview::Lyrics,
            LyricsGap | LyricsGapWaves | LyricsCountdown => Preview::Gap,
            NpMotion => Preview::Motion,
            NpVolume => Preview::Volume,
            _ => return None,
        })
    }
}

/// How long the glow on a linked row lasts.
pub const FOCUS_S: f64 = 1.5;
/// Frames the scroll to a linked row is repeated, while the page settles.
const SCROLL_FRAMES: u8 = 3;

/// The row a link opened, kept for its glow.
#[derive(Clone, Copy, Debug)]
struct Focus {
    id: SettingId,
    start: f64,
    scrolls: u8,
    rect: Option<Rect>,
}

/// What a row reports about its insides while it is drawn.
#[derive(Clone, Debug, Default)]
struct Parts {
    button: Option<Rect>,
    label: Option<Rect>,
    control: Option<Rect>,
    choices: Vec<Rect>,
}

/// One row drawn this pass, for the cards.
#[derive(Clone, Debug)]
struct Row {
    id: SettingId,
    label: String,
    rect: Rect,
    button: Rect,
    /// The name (with its ⓘ): hovering it shows every option.
    name: Rect,
    control: Option<Rect>,
    choices: Vec<Rect>,
    clip: Rect,
    hovered: bool,
}

#[derive(Clone, Debug, Default)]
struct Rows {
    pass: u64,
    list: Vec<Row>,
}

/// The card pinned open by a row's ⓘ.
#[derive(Clone, Copy, Debug)]
struct Pinned {
    id: SettingId,
    /// The pass it was opened in (its click is not a click outside).
    opened: u64,
    /// When it opened, for its fade-in.
    at: f64,
    /// Where the card was last drawn.
    card: Option<Rect>,
}

/// What a card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    /// The row's explanation (and preview).
    Row,
    /// One option of a choice, previewed.
    Option(usize),
    /// Every option side by side (the pinned card; read-only on hover).
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CardKey {
    id: SettingId,
    body: Body,
}

/// The hover card between frames: what it shows, the pointer's timing,
/// and its springs (rect as x, y, w, h; opacity; content opacity).
#[derive(Clone, Copy, Debug)]
struct Hover {
    key: Option<CardKey>,
    open: bool,
    /// What the pointer rests on while no card is open, and since when.
    candidate: Option<CardKey>,
    since: f64,
    /// When the pointer left every row (the card waits a little).
    left: Option<f64>,
    rect: Animated<[f32; 4]>,
    alpha: Animated<f32>,
    content: Animated<f32>,
    /// Where the card was drawn last frame.
    drawn: Option<Rect>,
}

fn focus_key() -> Id {
    Id::new("settings-focus")
}

fn parts_key() -> Id {
    Id::new("settings-help-parts")
}

fn rows_key() -> Id {
    Id::new("settings-help-rows")
}

fn pinned_key() -> Id {
    Id::new("settings-help-pinned")
}

fn hover_key() -> Id {
    Id::new("settings-help-hover")
}

fn pinned(ctx: &egui::Context) -> Option<Pinned> {
    ctx.data(|d| d.get_temp::<Option<Pinned>>(pinned_key())).flatten()
}

fn set_pinned(ctx: &egui::Context, p: Option<Pinned>) {
    ctx.data_mut(|d| d.insert_temp(pinned_key(), p));
}

/// Starts the scroll and glow for `id` (the page then switches its tab).
pub fn begin_focus(ctx: &egui::Context, id: SettingId) {
    let start = anim::now(ctx);
    ctx.data_mut(|d| d.insert_temp(focus_key(), Focus { id, start, scrolls: SCROLL_FRAMES, rect: None }));
}

/// The glow (0..1) `t` seconds after a link opened the row: two soft
/// pulses fading out by [`FOCUS_S`].
pub fn glow_at(t: f32) -> f32 {
    let total = FOCUS_S as f32;
    if !(0.0..total).contains(&t) {
        return 0.0;
    }
    let rise = (t / 0.15).clamp(0.0, 1.0);
    let fall = 1.0 - (t / total).powi(2);
    let pulse = 0.7 + 0.3 * (t * std::f32::consts::TAU / 0.75).cos();
    rise * fall * pulse
}

/// Wraps one settings item: the glow behind it when a link opened it, its
/// ⓘ button (unless the item placed it itself with [`help_button`]) and
/// the cards about it.
pub fn item<R>(ui: &mut Ui, id: SettingId, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    ctx.data_mut(|d| d.insert_temp(parts_key(), Parts::default()));
    let bg = ui.painter().add(Shape::Noop);
    let inner = ui.scope(add);
    let rect = inner.response.rect;
    if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key()))
        && f.id == id
    {
        let t = (anim::now(&ctx) - f.start) as f32;
        let a = glow_at(t);
        if a > 0.0 {
            let s = Theme::get(&ctx).scheme;
            // The whole row of the column, with room around the text. It
            // may reach past the page's left edge, so only the vertical clip
            // of the scroll area applies.
            let row = Rect::from_x_y_ranges(ui.max_rect().x_range(), rect.y_range());
            let r = row.expand2(vec2(12.0, 7.0));
            let clip = ui.clip_rect();
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(Rect::from_x_y_ranges(r.x_range(), clip.y_range()));
            painter.set(bg, Shape::rect_filled(r, shape::LARGE, s.primary.gamma_multiply(0.18 * a)));
            painter.rect_stroke(r, shape::LARGE, Stroke::new(2.0, s.primary.gamma_multiply(0.7 * a)), egui::StrokeKind::Inside);
            ctx.request_repaint();
        }
        f.rect = Some(rect);
        ctx.data_mut(|d| d.insert_temp(focus_key(), f));
    }
    let parts = ctx.data_mut(|d| d.remove_temp::<Parts>(parts_key())).unwrap_or_default();
    let button = parts.button.unwrap_or_else(|| {
        // Blocks of fields: beside the first field, on its centre line.
        let r = Rect::from_min_size(pos2(rect.right() + 8.0, rect.top() + 17.0), Vec2::splat(BTN));
        help_button_at(ui, id, r);
        r
    });
    let row = Row {
        id,
        label: label.to_owned(),
        rect: rect.union(button),
        button,
        name: parts.label.map_or(button, |l| l.union(button)),
        control: parts.control,
        choices: parts.choices,
        clip: ui.clip_rect(),
        hovered: ui.rect_contains_pointer(rect.union(button)),
    };
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let rows = d.get_temp_mut_or_default::<Rows>(rows_key());
        if rows.pass != pass {
            rows.pass = pass;
            rows.list.clear();
        }
        rows.list.push(row);
    });
    inner.inner
}

/// Size of the ⓘ button.
pub const BTN: f32 = 22.0;

/// The ⓘ button of the item being drawn, placed here in the layout (next
/// to the row's label).
pub fn help_button(ui: &mut Ui, id: SettingId) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(BTN), Sense::hover());
    help_button_at(ui, id, rect);
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).button = Some(rect));
}

/// The rect of the item's name, so hovering it shows every option.
pub fn report_label(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).label = Some(rect));
}

/// The rect of the item's control, so cards line up with it.
pub fn report_control(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).control = Some(rect));
}

/// The rects of the item's options, in [`SettingId::choices`] order, so
/// hovering one previews it.
pub fn report_choices(ctx: &egui::Context, rects: Vec<Rect>) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).choices = rects);
}

fn help_button_at(ui: &Ui, id: SettingId, rect: Rect) {
    let ctx = ui.ctx();
    let s = Theme::get(ctx).scheme;
    let resp = ui
        .interact(rect, Id::new(("settings-help-btn", id)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let was = pinned(ctx).is_some_and(|p| p.id == id);
    if resp.clicked() {
        let at = anim::now(ctx);
        set_pinned(ctx, (!was).then(|| Pinned { id, opened: ctx.cumulative_pass_nr(), at, card: None }));
    }
    let on = was != resp.clicked();
    let painter = ui.painter();
    if on {
        painter.circle_filled(rect.center(), BTN * 0.5, s.secondary_container);
    } else if resp.hovered() {
        painter.circle_filled(rect.center(), BTN * 0.5, s.on_surface.gamma_multiply(0.08));
    }
    let tint = if on {
        s.on_secondary_container
    } else if resp.hovered() {
        s.on_surface
    } else {
        s.on_surface_variant.gamma_multiply(0.75)
    };
    Icon::Info.widget(on, tint, 16.0).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(16.0)));
}

/// After the page: scrolls to a linked row and shows the pinned card, or
/// the hover card for what the pointer rests on. A pick in the pinned card
/// lands in `prefs`.
pub fn finish(ui: &mut Ui, prefs: &mut Prefs) {
    let ctx = ui.ctx().clone();
    if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key())) {
        if let Some(rect) = f.rect.take()
            && f.scrolls > 0
        {
            // After the controls, so their nested scroll areas cannot take it.
            ui.scroll_to_rect(rect.expand(24.0), Some(Align::Center));
            f.scrolls -= 1;
            ctx.request_repaint();
        }
        if f.scrolls == 0 && anim::now(&ctx) - f.start > FOCUS_S {
            ctx.data_mut(|d| d.remove::<Focus>(focus_key()));
        } else {
            ctx.data_mut(|d| d.insert_temp(focus_key(), f));
        }
    }

    let pass = ctx.cumulative_pass_nr();
    let rows = ctx.data(|d| d.get_temp::<Rows>(rows_key())).filter(|r| r.pass == pass).map(|r| r.list).unwrap_or_default();

    if let Some(mut pin) = pinned(&ctx) {
        // The hover card gives way at once.
        ctx.data_mut(|d| d.remove::<Hover>(hover_key()));
        // Gone with its row (another tab, hidden, scrolled away).
        let Some(row) = rows.iter().find(|r| r.id == pin.id && r.clip.intersects(r.rect)) else {
            set_pinned(&ctx, None);
            return;
        };
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let outside = pin.opened != pass
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !row.rect.contains(p) && !pin.card.is_some_and(|c| c.contains(p)))
            });
        if esc || outside {
            set_pinned(&ctx, None);
            return;
        }
        let layout = Layout::new(&ctx, row, Body::All, true);
        let target = Rect::from_min_size(place(row.rect, layout.anchor_x, layout.size, ctx.content_rect()), layout.size);
        // A short fade and rise from the row (effects timing, no bounce).
        let t = ((anim::now(&ctx) - pin.at) / 0.18).clamp(0.0, 1.0) as f32;
        let k = 1.0 - (1.0 - t).powi(3);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let rise = if target.top() >= row.rect.bottom() { -1.0 } else { 1.0 } * 8.0 * (1.0 - k);
        let rect = target.translate(vec2(0.0, -rise));
        let out = show_card(&ctx, Id::new("settings-help-pinned-card"), rect, k, 1.0, &layout, row, prefs, true);
        if let Some(i) = out.pick {
            row.id.choose(prefs, i);
        }
        pin.card = Some(rect);
        set_pinned(&ctx, (!out.close).then_some(pin));
        return;
    }
    hover(&ctx, &rows, prefs);
}

/// First card after the pointer rests this long on something.
const ENTER_DELAY: f64 = 0.25;
/// The card waits this long after the pointer leaves.
const LEAVE_GRACE: f64 = 0.15;

/// What the pointer at `p` asks for in `row`.
fn key_at(row: &Row, p: Pos2) -> CardKey {
    let n = row.id.choices().len();
    let body = match row.choices.iter().position(|r| r.contains(p)).filter(|i| *i < n) {
        Some(i) => Body::Option(i),
        None if n > 0 && row.name.contains(p) => Body::All,
        None => Body::Row,
    };
    CardKey { id: row.id, body }
}

/// The hover card: a short wait before the first one (none while the
/// pointer only passes by), then it follows the pointer between rows and
/// options at once, gliding and cross-fading; it stays while the pointer
/// is on it and a moment after the pointer leaves.
fn hover(ctx: &egui::Context, rows: &[Row], prefs: &mut Prefs) {
    let theme = Theme::get(ctx);
    let now = anim::now(ctx);
    let spatial = anim::effective(ctx, theme.motion.fast_spatial.without_bounce());
    let effects = anim::effective(ctx, theme.motion.fast_effects);
    let mut h = ctx.data(|d| d.get_temp::<Hover>(hover_key())).unwrap_or(Hover {
        key: None,
        open: false,
        candidate: None,
        since: now,
        left: None,
        rect: Animated::new([0.0; 4], spatial),
        alpha: Animated::new(0.0, effects),
        content: Animated::new(1.0, effects),
        drawn: None,
    });
    let pointer = ctx.pointer_hover_pos();
    // On the card itself: keep what it shows.
    let on_card = h.open && pointer.is_some_and(|p| h.drawn.is_some_and(|r| r.contains(p)));
    let want = if on_card {
        h.key
    } else {
        pointer.and_then(|p| rows.iter().find(|r| r.hovered).map(|r| key_at(r, p)))
    };
    match want {
        Some(k) => {
            h.left = None;
            if h.open || h.alpha.value(now) > 0.05 {
                if h.key != Some(k) {
                    // Another row or option while a card is up: switch now.
                    if h.key.is_some() {
                        h.content.snap(0.0);
                        h.content.set_target(1.0, now);
                    }
                    h.key = Some(k);
                }
                if !h.open {
                    h.open = true;
                    h.alpha.set_target(1.0, now);
                }
            } else {
                if h.candidate != Some(k) {
                    h.candidate = Some(k);
                    h.since = now;
                }
                let wait = ENTER_DELAY - (now - h.since);
                if wait <= 0.0 {
                    h.key = Some(k);
                    h.open = true;
                    h.drawn = None;
                    h.content.snap(1.0);
                    h.alpha.snap(0.0);
                    h.alpha.set_target(1.0, now);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
                }
            }
        }
        None => {
            h.candidate = None;
            if h.open {
                let left = *h.left.get_or_insert(now);
                if now - left >= LEAVE_GRACE {
                    h.open = false;
                    h.alpha.set_target(0.0, now);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(LEAVE_GRACE - (now - left)));
                }
            }
        }
    }

    let alpha = h.alpha.value(now).clamp(0.0, 1.0);
    let row = h.key.and_then(|k| rows.iter().find(|r| r.id == k.id).map(|r| (k, r)));
    match row {
        Some((key, row)) if alpha > 0.005 || h.open => {
            let layout = Layout::new(ctx, row, key.body, false);
            let target = Rect::from_min_size(place(row.rect, layout.anchor_x, layout.size, ctx.content_rect()), layout.size);
            let goal = [target.left(), target.top(), target.width(), target.height()];
            if h.drawn.is_none() {
                // Appearing: rise a little from the row's side.
                let rise = if target.top() >= row.rect.bottom() { -8.0 } else { 8.0 };
                h.rect.snap([goal[0], goal[1] - rise, goal[2], goal[3]]);
            }
            h.rect.set_target(goal, now);
            let [x, y, w, hh] = h.rect.value(now);
            let rect = Rect::from_min_size(pos2(x, y), vec2(w, hh));
            let content = h.content.value(now).clamp(0.0, 1.0);
            show_card(ctx, Id::new("settings-help-card"), rect, alpha, content, &layout, row, prefs, false);
            h.drawn = Some(rect);
            if !(h.rect.is_settled(now) && h.alpha.is_settled(now) && h.content.is_settled(now)) {
                ctx.request_repaint();
            }
        }
        _ => {
            h.key = if h.open { h.key } else { None };
            h.drawn = None;
        }
    }
    ctx.data_mut(|d| d.insert_temp(hover_key(), h));
}

/// Text width of a one-thing card.
const CARD_W: f32 = 280.0;
const PREVIEW_H: f32 = 120.0;
const PAD: f32 = 12.0;
/// Between a row and its card.
const GAP: f32 = 6.0;
const TILE_GAP: f32 = 8.0;
const HINT: &str = "Наведи на название — все варианты рядом";

/// A paragraph in `role`, wrapped at `width`.
fn para(painter: &Painter, role: TypeRole, text: &str, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), role.style().text_format(color));
    job.wrap.max_width = width;
    painter.layout_job(job)
}

/// Where a card of `size` goes for `row`: just under it, from `x`, or just
/// above when there is no room below; inside `screen`, and over the row
/// only when it fits neither way.
fn place(row: Rect, x: f32, size: Vec2, screen: Rect) -> Pos2 {
    // Keep off the window's edges: more at the sides than at the bottom,
    // where a few px decide whether the card fits under its row.
    let screen = screen.shrink2(vec2(8.0, 4.0));
    let x = x.clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    let below = row.bottom() + GAP;
    let above = row.top() - GAP - size.y;
    let y = if below + size.y <= screen.bottom() {
        below
    } else if above >= screen.top() {
        above
    } else if screen.bottom() - row.bottom() >= row.top() - screen.top() {
        (screen.bottom() - size.y).max(screen.top())
    } else {
        screen.top()
    };
    pos2(x, y)
}

fn card_frame(painter: &Painter, rect: Rect, s: &ColorScheme) {
    let radii = Radii::all(shape::MEDIUM);
    paint::shadow(painter, rect, radii, Elevation::Level2);
    paint::container(painter, rect, radii, s.surface_container, Stroke::NONE);
}

/// Prefs with option `i` of `id` picked.
fn with_choice(id: SettingId, prefs: &Prefs, i: usize) -> Prefs {
    let mut p = prefs.clone();
    id.choose(&mut p, i);
    p
}

/// Tile size of a preview kind in the grid of options.
fn tile_size(kind: Preview) -> Vec2 {
    match kind {
        Preview::Palette => vec2(124.0, 64.0),
        Preview::Ambient => vec2(150.0, 90.0),
        Preview::Cover | Preview::Ring => vec2(124.0, 124.0),
        Preview::Background => vec2(168.0, 104.0),
        Preview::Lyrics => vec2(264.0, 100.0),
        Preview::Gap => vec2(124.0, 100.0),
        Preview::Motion => vec2(150.0, 100.0),
        Preview::Volume => vec2(150.0, 80.0),
        Preview::Wave => vec2(150.0, 60.0),
    }
}

/// A card laid out: its size, where it lines up, and its texts.
struct Layout {
    size: Vec2,
    anchor_x: f32,
    body: Body,
    kind: Option<Preview>,
    title: Arc<Galley>,
    help: Arc<Galley>,
    hint: Option<Arc<Galley>>,
    /// [`Body::All`]: each option's name and explanation.
    texts: Vec<(Arc<Galley>, Arc<Galley>)>,
    /// [`Body::All`] with previews: tile size, columns and each grid row's
    /// text height.
    tiles: Option<(Preview, Vec2, usize, Vec<f32>)>,
    /// Content width and header height.
    w: f32,
    header_h: f32,
    close: bool,
}

impl Layout {
    fn new(ctx: &egui::Context, row: &Row, body: Body, pinned: bool) -> Self {
        let s = Theme::get(ctx).scheme;
        let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("settings-help-measure")));
        let choices = row.id.choices();
        let kind = row.id.preview();
        match body {
            Body::Row | Body::Option(_) => {
                let (title, help) = match body {
                    Body::Option(i) => (format!("{} · {}", row.label, choices[i].label), choices[i].help),
                    _ => (row.label.clone(), row.id.help()),
                };
                let title = para(&painter, TypeRole::TitleSmall, &title, s.on_surface, CARD_W);
                let help = para(&painter, TypeRole::BodyMedium, help, s.on_surface_variant, CARD_W);
                let hint = (body == Body::Row && !choices.is_empty())
                    .then(|| para(&painter, TypeRole::LabelMedium, HINT, s.primary, CARD_W));
                let h = PAD
                    + kind.map_or(0.0, |_| PREVIEW_H + 10.0)
                    + title.size().y
                    + 2.0
                    + help.size().y
                    + hint.as_ref().map_or(0.0, |g| 6.0 + g.size().y)
                    + PAD;
                let anchor_x = match body {
                    Body::Option(i) => row.choices.get(i).map(|r| r.left()),
                    _ => None,
                }
                .or(row.control.map(|r| r.left()))
                .unwrap_or(row.button.left());
                Self {
                    size: vec2(CARD_W + 2.0 * PAD, h),
                    anchor_x,
                    body,
                    kind,
                    title,
                    help,
                    hint,
                    texts: Vec::new(),
                    tiles: None,
                    w: CARD_W,
                    header_h: 0.0,
                    close: false,
                }
            }
            Body::All => {
                let screen = ctx.content_rect();
                let grid = (!choices.is_empty()).then_some(kind).flatten();
                let (tile, cols) = match grid {
                    Some(k) => {
                        let tile = tile_size(k);
                        let fit = ((screen.width() - 16.0 - 2.0 * PAD + TILE_GAP) / (tile.x + TILE_GAP)).floor().max(1.0) as usize;
                        (tile, choices.len().min(4).min(fit))
                    }
                    None => (Vec2::ZERO, 1),
                };
                let inner_w = if grid.is_some() { cols as f32 * tile.x + (cols - 1) as f32 * TILE_GAP } else { 320.0 };
                let w = inner_w.max(280.0);
                let close_w = if pinned { 36.0 } else { 0.0 };
                let title = para(&painter, TypeRole::TitleMedium, &row.label, s.on_surface, w - close_w);
                let help = para(&painter, TypeRole::BodyMedium, row.id.help(), s.on_surface_variant, w);
                let texts: Vec<(Arc<Galley>, Arc<Galley>)> = choices
                    .iter()
                    .map(|c| {
                        let tw = if grid.is_some() { tile.x - 4.0 } else { w - 40.0 };
                        (
                            para(&painter, TypeRole::LabelLarge, c.label, s.on_surface, tw),
                            para(&painter, TypeRole::BodySmall, c.help, s.on_surface_variant, tw),
                        )
                    })
                    .collect();
                let text_h = |i: usize| texts[i].0.size().y + 2.0 + texts[i].1.size().y;
                let header_h = title.size().y.max(if pinned { 28.0 } else { 0.0 });
                let mut h = PAD + header_h + 4.0 + help.size().y + 12.0;
                let mut grid_rows = Vec::new();
                if grid.is_some() {
                    for r in 0..choices.len().div_ceil(cols) {
                        let th = (r * cols..((r + 1) * cols).min(choices.len())).map(text_h).fold(0.0, f32::max);
                        grid_rows.push(th);
                        h += tile.y + 6.0 + th + 8.0 + TILE_GAP;
                    }
                    h -= TILE_GAP;
                } else if !choices.is_empty() {
                    for i in 0..choices.len() {
                        h += text_h(i) + 16.0;
                    }
                } else if kind.is_some() {
                    h += PREVIEW_H;
                }
                h += PAD;
                Self {
                    size: vec2(w + 2.0 * PAD, h),
                    anchor_x: row.control.map_or(row.button.left(), |r| r.left().min(row.button.left())),
                    body,
                    kind,
                    title,
                    help,
                    hint: None,
                    texts,
                    tiles: grid.map(|k| (k, tile, cols, grid_rows)),
                    w,
                    header_h,
                    close: pinned,
                }
            }
        }
    }
}

/// What a card's clicks did.
struct CardOut {
    pick: Option<usize>,
    close: bool,
}

/// Paints `layout` into `rect` (which may still be gliding to its size) at
/// `alpha`, the content at `content`; `interactive` cards take clicks.
#[allow(clippy::too_many_arguments)]
fn show_card(
    ctx: &egui::Context,
    area: Id,
    rect: Rect,
    alpha: f32,
    content: f32,
    layout: &Layout,
    row: &Row,
    prefs: &Prefs,
    interactive: bool,
) -> CardOut {
    let s = Theme::get(ctx).scheme;
    let mut out = CardOut { pick: None, close: false };
    Area::new(area)
        .order(Order::Tooltip)
        .fixed_pos(rect.min)
        .constrain(false)
        .interactable(interactive)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            let sense = if interactive { Sense::click() } else { Sense::hover() };
            let (_, _) = ui.allocate_exact_size(rect.size(), sense);
            card_frame(ui.painter(), rect, &s);
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.multiply_opacity(content);
            let ui = &mut ui;
            let x0 = rect.left() + PAD;
            let mut y = rect.top() + PAD;
            match layout.body {
                Body::Row | Body::Option(_) => {
                    if let Some(kind) = layout.kind {
                        let r = Rect::from_min_size(pos2(x0, y), vec2(CARD_W, PREVIEW_H));
                        match layout.body {
                            Body::Option(i) => {
                                sample(ui, r, kind, &with_choice(row.id, prefs, i), Some(Id::new(("hover", row.id, i))));
                            }
                            _ => sample(ui, r, kind, prefs, None),
                        }
                        y += PREVIEW_H + 10.0;
                    }
                    let painter = ui.painter();
                    painter.galley(pos2(x0, y), layout.title.clone(), s.on_surface);
                    y += layout.title.size().y + 2.0;
                    painter.galley(pos2(x0, y), layout.help.clone(), s.on_surface_variant);
                    y += layout.help.size().y + 6.0;
                    if let Some(g) = &layout.hint {
                        painter.galley(pos2(x0, y), g.clone(), s.primary);
                    }
                }
                Body::All => {
                    out = all_options(ui, rect, layout, row, prefs, interactive);
                }
            }
        });
    out
}

/// The body of a [`Body::All`] card: header, explanation and the options.
fn all_options(ui: &mut Ui, rect: Rect, l: &Layout, row: &Row, prefs: &Prefs, interactive: bool) -> CardOut {
    let s = Theme::get(ui.ctx()).scheme;
    let mut out = CardOut { pick: None, close: false };
    let area = Id::new(("settings-help-all", row.id));
    let chosen = row.id.chosen(prefs);
    let x0 = rect.left() + PAD;
    let mut y = rect.top() + PAD;
    let tg = l.title.size().y;
    ui.painter().galley(pos2(x0, y + (l.header_h - tg) * 0.5), l.title.clone(), s.on_surface);
    if l.close {
        let close_r = Rect::from_center_size(pos2(rect.right() - PAD - 14.0, y + l.header_h * 0.5), Vec2::splat(28.0));
        let resp = ui.interact(close_r, area.with("close"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if resp.hovered() {
            ui.painter().circle_filled(close_r.center(), 14.0, s.on_surface.gamma_multiply(0.08));
        }
        Icon::Close.widget(false, s.on_surface_variant, 18.0).paint_at(ui, Rect::from_center_size(close_r.center(), Vec2::splat(18.0)));
        out.close = resp.clicked();
    }
    y += l.header_h + 4.0;
    ui.painter().galley(pos2(x0, y), l.help.clone(), s.on_surface_variant);
    y += l.help.size().y + 12.0;
    let sense = if interactive { Sense::click() } else { Sense::hover() };
    let text_h = |i: usize| l.texts[i].0.size().y + 2.0 + l.texts[i].1.size().y;

    if let Some((k, tile, cols, grid_rows)) = &l.tiles {
        let (tile, cols) = (*tile, *cols);
        for (r, th) in grid_rows.iter().enumerate() {
            for col in 0..cols {
                let i = r * cols + col;
                if i >= l.texts.len() {
                    break;
                }
                let tx = x0 + col as f32 * (tile.x + TILE_GAP);
                let whole = Rect::from_min_size(pos2(tx, y), vec2(tile.x, tile.y + 6.0 + th + 8.0)).expand(4.0);
                let mut resp = ui.interact(whole, area.with(("tile", i)), sense);
                if interactive {
                    resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                }
                let sel = i == chosen;
                if sel {
                    ui.painter().rect_filled(whole, shape::SMALL + 4.0, s.secondary_container.gamma_multiply(0.6));
                } else if interactive && resp.hovered() {
                    ui.painter().rect_filled(whole, shape::SMALL + 4.0, s.on_surface.gamma_multiply(0.06));
                }
                let pr = Rect::from_min_size(pos2(tx, y), tile);
                sample(ui, pr, *k, &with_choice(row.id, prefs, i), Some(area.with(("tile", i))));
                if sel {
                    ui.painter().rect_stroke(pr, shape::SMALL, Stroke::new(2.0, s.primary), egui::StrokeKind::Inside);
                }
                let (lg, hg) = l.texts[i].clone();
                let lh = lg.size().y;
                ui.painter().galley(pos2(tx + 2.0, pr.bottom() + 6.0), lg, if sel { s.primary } else { s.on_surface });
                ui.painter().galley(pos2(tx + 2.0, pr.bottom() + 8.0 + lh), hg, s.on_surface_variant);
                if interactive && resp.clicked() && !sel {
                    out.pick = Some(i);
                }
            }
            y += tile.y + 6.0 + th + 8.0 + TILE_GAP;
        }
    } else if !l.texts.is_empty() {
        // A list: a radio mark, the option and what it does.
        for (i, (lg, hg)) in l.texts.iter().enumerate() {
            let rh = text_h(i) + 16.0;
            let r = Rect::from_min_size(pos2(x0 - 4.0, y), vec2(l.w + 8.0, rh));
            let mut resp = ui.interact(r, area.with(("opt", i)), sense);
            if interactive {
                resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            }
            let sel = i == chosen;
            if sel {
                ui.painter().rect_filled(r, shape::SMALL, s.secondary_container.gamma_multiply(0.6));
            } else if interactive && resp.hovered() {
                ui.painter().rect_filled(r, shape::SMALL, s.on_surface.gamma_multiply(0.06));
            }
            let dot = pos2(x0 + 10.0, y + 8.0 + lg.size().y * 0.5);
            ui.painter().circle_stroke(dot, 7.0, Stroke::new(2.0, if sel { s.primary } else { s.on_surface_variant }));
            if sel {
                ui.painter().circle_filled(dot, 3.5, s.primary);
            }
            let lh = lg.size().y;
            ui.painter().galley(pos2(x0 + 28.0, y + 8.0), lg.clone(), if sel { s.primary } else { s.on_surface });
            ui.painter().galley(pos2(x0 + 28.0, y + 10.0 + lh), hg.clone(), s.on_surface_variant);
            if interactive && resp.clicked() && !sel {
                out.pick = Some(i);
            }
            y += rh;
        }
    } else if let Some(k) = l.kind {
        sample(ui, Rect::from_min_size(pos2(x0, y), vec2(l.w, PREVIEW_H)), k, prefs, None);
    }
    out
}

/// A preview in its own box: `salt` marks an option's sample (an option
/// that turns the effect off then says so instead of showing a stand-in).
fn sample(ui: &mut Ui, rect: Rect, kind: Preview, prefs: &Prefs, salt: Option<Id>) {
    let palette = (kind == Preview::Palette && salt.is_some()).then(|| scheme_for(ui.ctx(), prefs));
    let bg = palette.map_or_else(|| Theme::get(ui.ctx()).scheme.surface, |s| s.surface);
    ui.painter().rect_filled(rect, shape::SMALL, bg);
    let mut pui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    pui.set_clip_rect(rect.intersect(ui.clip_rect()));
    paint_preview(&mut pui, rect, kind, prefs, salt, palette);
}

/// The scheme `prefs` would give the current cover (for the palette samples
/// of other themes, contrasts and variants), cached.
fn scheme_for(ctx: &egui::Context, prefs: &Prefs) -> ColorScheme {
    type Key = (u32, bool, SchemeVariant, Contrast);
    let seed = ctx.data(|d| d.get_temp::<u32>(crate::screens::scheme_seed_id())).unwrap_or(prefs.fallback_seed);
    let (dark, variant, contrast) = crate::screens::theme_key(ctx, prefs);
    let key: Key = (seed, dark, variant, contrast);
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<HashMap<Key, ColorScheme>>(Id::new("settings-help-schemes"));
        *cache.entry(key).or_insert_with(|| ColorScheme::from_seed(seed, variant, dark, contrast))
    })
}

fn off_label(ui: &Ui, rect: Rect) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Выключено", TypeRole::LabelLarge.style().font_id(), s.on_surface_variant);
}

fn paint_preview(ui: &mut Ui, rect: Rect, kind: Preview, prefs: &Prefs, salt: Option<Id>, palette: Option<ColorScheme>) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let exact = salt.is_some();
    match kind {
        Preview::Palette => {
            // The scheme the interface paints with (or would, for an option).
            let s = palette.unwrap_or(s);
            let swatches = [s.primary, s.secondary, s.tertiary, s.primary_container, s.surface_container_highest];
            let small = rect.height() < 100.0;
            let pad = if small { 6.0 } else { 8.0 };
            let w = (rect.width() - 2.0 * pad) / swatches.len() as f32;
            let sh = if small { rect.height() * 0.42 } else { 64.0 };
            for (i, c) in swatches.iter().enumerate() {
                let r = Rect::from_min_size(pos2(rect.left() + pad + i as f32 * w, rect.top() + pad), vec2(w - 4.0, sh));
                ui.painter().rect_filled(r, if small { 8.0 } else { shape::MEDIUM }, *c);
            }
            let bh = if small { rect.height() - sh - 3.0 * pad } else { 32.0 };
            let text = Rect::from_min_size(pos2(rect.left() + pad + 4.0, rect.bottom() - pad - bh), vec2(rect.width() - 2.0 * pad - 8.0, bh));
            ui.painter().rect_filled(text, bh * 0.5, s.primary);
            let role = if small { TypeRole::LabelMedium } else { TypeRole::LabelLarge };
            ui.painter().text(text.center(), egui::Align2::CENTER_CENTER, "Кнопка", role.style().font_id(), s.on_primary);
        }
        Preview::Ambient => {
            if exact && prefs.ambient == v::Ambient::Off {
                off_label(ui, rect);
            } else {
                // Off (for the row): a sample of the calm glow.
                let t = anim::now(ui.ctx()) as f32;
                let bass = if prefs.ambient == v::Ambient::Reactive { beat(t) } else { 0.0 };
                crate::screens::ambient::paint(ui.ctx(), ui.painter(), rect, bass, true, prefs.ambient_strength);
            }
        }
        Preview::Wave => {
            let t = anim::now(ui.ctx()) as f32;
            let cy = rect.center().y;
            let (x0, x1) = (rect.left() + 20.0, rect.right() - 20.0);
            let head = x0 + (x1 - x0) * (0.25 + 0.5 * ((t * 0.1).fract()));
            let wavy = prefs.progress_wave && !theme.reduced_motion;
            let amp = if wavy { 3.0 } else { 0.0 };
            let phase = (t * 0.6).fract() * std::f32::consts::TAU;
            wave_line(ui, x0, head, cy, amp, 40.0, phase, Stroke::new(4.0, s.primary));
            ui.painter().line_segment([pos2(head + 8.0, cy), pos2(x1, cy)], Stroke::new(4.0, s.secondary_container));
            ui.painter().circle_filled(pos2(x1, cy), 2.0, s.primary);
        }
        Preview::Cover => now_playing::preview(ui, rect, prefs, false),
        Preview::Ring => now_playing::preview(ui, rect, prefs, prefs.ring),
        Preview::Background => {
            if exact && prefs.bg_shapes == v::BgShapes::Off {
                off_label(ui, rect);
            } else {
                // Off (for the row and its fields): a sample of how they
                // would float.
                let sample;
                let prefs = if prefs.bg_shapes == v::BgShapes::Off {
                    sample = Prefs { bg_shapes: v::BgShapes::Float, ..prefs.clone() };
                    &sample
                } else {
                    prefs
                };
                // A slower beat than the others: one a second, so bouncing
                // shapes land between the hops.
                let t = anim::now(ui.ctx()) as f32;
                let kick = beat(t * 0.5);
                let transients: Vec<f32> = (0..8).map(|i| kick * (0.6 + 0.4 * (i as f32 * 1.7 + t).sin())).collect();
                bg_shapes::preview(ui, rect, prefs, kick, &transients, salt.unwrap_or(Id::new("row")));
            }
        }
        Preview::Lyrics => lyrics::preview(ui, rect, prefs),
        Preview::Gap => lyrics::gap_preview(ui, rect, prefs),
        Preview::Motion => motion_preview(ui, rect, prefs),
        Preview::Volume => volume_preview(ui, rect, prefs),
    }
    ui.ctx().request_repaint();
}

/// Now Playing opening from the player bar and closing back, on loop, with
/// the spring of `prefs.np_motion`.
fn motion_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let spring = anim::effective(ui.ctx(), crate::screens::np_spring(prefs.np_motion));
    // Opens, stays open a while, closes, rests a little.
    const OPEN: f32 = 1.8;
    const LOOP: f32 = 2.8;
    let t = (anim::now(ui.ctx()) as f32).rem_euclid(LOOP);
    let opening = t < OPEN;
    let (x, _) = spring.solve(1.0, 0.0, if opening { t } else { t - OPEN });
    let k = if opening { 1.0 - x } else { x };
    let pad = (rect.height() * 0.1).min(12.0);
    let bar_h = (rect.height() * 0.22).clamp(14.0, 28.0);
    let bar = Rect::from_min_max(pos2(rect.left() + pad, rect.bottom() - pad - bar_h), pos2(rect.right() - pad, rect.bottom() - pad));
    let full = rect.shrink(pad * 0.5);
    let lerp = |a: f32, b: f32| a + (b - a) * k;
    let open = Rect::from_min_max(pos2(lerp(bar.left(), full.left()), lerp(bar.top(), full.top())), pos2(lerp(bar.right(), full.right()), lerp(bar.bottom(), full.bottom())));
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.rect_filled(bar, bar_h * 0.5, s.surface_container_highest);
    painter.rect_filled(open, lerp(bar_h * 0.5, 10.0), s.surface_container_high);
    // The cover grows from the bar's thumbnail to the middle of the screen.
    let side = lerp(bar_h - 8.0, open.height() * 0.55).max(4.0);
    let c = pos2(lerp(open.left() + 4.0 + side * 0.5, open.center().x), lerp(open.center().y, open.top() + open.height() * 0.42));
    painter.rect_filled(Rect::from_center_size(c, Vec2::splat(side)), lerp(4.0, 8.0), s.primary);
}

/// The player's volume control under `prefs.np_volume` /
/// `np_volume_pop`: a slider beside the sound button, a pill rising at the
/// bottom while the volume changes, or the button alone.
fn volume_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let t = (anim::now(ui.ctx()) as f32).rem_euclid(3.0);
    // The volume drifts up and down while it is "being changed".
    let level = 0.35 + 0.3 * (t * 2.1).sin().abs();
    let pad = (rect.height() * 0.14).clamp(6.0, 14.0);
    let r = (rect.height() * 0.12).clamp(7.0, 12.0);
    let button = pos2(rect.left() + pad + r, rect.top() + pad + r);
    painter.circle_filled(button, r, s.surface_container_highest);
    Icon::VolumeUp.widget(false, s.on_surface_variant, r * 1.3).paint_at(ui, Rect::from_center_size(button, Vec2::splat(r * 1.3)));
    let bar = |painter: &Painter, a: Pos2, len: f32, alpha: f32| {
        let b = a + vec2(len, 0.0);
        painter.line_segment([a, b], Stroke::new(4.0, s.secondary_container.gamma_multiply(alpha)));
        painter.line_segment([a, a + vec2(len * level, 0.0)], Stroke::new(4.0, s.primary.gamma_multiply(alpha)));
        painter.circle_filled(a + vec2(len * level, 0.0), 5.0, s.primary.gamma_multiply(alpha));
    };
    if prefs.np_volume {
        bar(&painter, button + vec2(r + 8.0, 0.0), (rect.right() - pad - button.x - r - 8.0).max(10.0), 1.0);
    } else if prefs.np_volume_pop {
        // Rises for two seconds of every three.
        let k = ((t / 0.25).min((2.0 - t) / 0.25)).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        let w = (rect.width() * 0.6).min(160.0);
        let h = 20.0;
        let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - pad - h * 0.5 + (1.0 - k) * 10.0), vec2(w, h));
        painter.rect_filled(pill, h * 0.5, s.surface_container_highest.gamma_multiply(k));
        bar(&painter, pos2(pill.left() + 12.0, pill.center().y), w - 24.0, k);
    } else {
        // The wheel: a small mouse beside the button.
        let m = Rect::from_center_size(button + vec2(r + 16.0, 0.0), vec2(12.0, 18.0));
        painter.rect_stroke(m, 6.0, Stroke::new(1.5, s.on_surface_variant), egui::StrokeKind::Inside);
        let wy = m.top() + 4.0 + 3.0 * (t * 4.0).sin().abs();
        painter.line_segment([pos2(m.center().x, wy), pos2(m.center().x, wy + 3.0)], Stroke::new(2.0, s.primary));
    }
}

/// A made-up 120 BPM beat (1 on the beat, decaying).
pub(crate) fn beat(t: f32) -> f32 {
    (-(t * 2.0).fract() * 6.0).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glow_pulses_then_fades_out() {
        assert_eq!(glow_at(-0.1), 0.0);
        assert!(glow_at(0.0) < 0.05);
        assert!(glow_at(0.2) > 0.5);
        // A dip between the two pulses.
        assert!(glow_at(0.375) < glow_at(0.75));
        assert!(glow_at(1.4) < 0.2);
        assert_eq!(glow_at(FOCUS_S as f32), 0.0);
    }

    #[test]
    fn linked_rows_know_their_tab_and_every_row_has_help() {
        assert_eq!(SettingId::LyricsStyle.tab(), TAB_APPEARANCE);
        assert_eq!(SettingId::BgHue.tab(), TAB_APPEARANCE);
        assert_eq!(SettingId::Prowlarr.tab(), TAB_GENERAL);
        assert_eq!(SettingId::NoSeed.tab(), TAB_GENERAL);
        for id in [SettingId::Design, SettingId::FpsLimit, SettingId::Nodes, SettingId::RingBeat] {
            assert!(!id.help().is_empty());
        }
    }

    #[test]
    fn every_choice_round_trips_through_prefs() {
        use SettingId as S;
        for id in [
            S::Theme, S::Contrast, S::Scheme, S::CoverShape, S::CoverDir, S::RingDir, S::RingStyle, S::Ambient,
            S::BgShapes, S::BgColors, S::LyricsStyle, S::WaveFallback, S::LyricsGap, S::NpMotion, S::DblClickShape,
            S::OpusBitrate,
        ] {
            assert!(id.choices().len() >= 2, "{id:?}");
            for i in 0..id.choices().len() {
                let mut p = Prefs::default();
                id.choose(&mut p, i);
                assert_eq!(id.chosen(&p), i, "{id:?} option {i}");
                assert!(!id.choices()[i].help.is_empty());
            }
        }
    }

    #[test]
    fn cards_sit_under_the_row_or_flip_above_and_stay_on_screen() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let size = vec2(300.0, 200.0);
        // Room below: just under the row, from the anchor.
        let row = Rect::from_min_max(pos2(100.0, 100.0), pos2(900.0, 156.0));
        let p = place(row, 400.0, size, screen);
        assert_eq!(p, pos2(400.0, 156.0 + GAP));
        // No room below: above, never over the row.
        let low = Rect::from_min_max(pos2(100.0, 650.0), pos2(900.0, 706.0));
        let p = place(low, 400.0, size, screen);
        assert!(p.y + size.y <= low.top());
        // Pushed in from the right edge.
        let p = place(row, 1100.0, size, screen);
        assert!(p.x + size.x <= screen.right() - 8.0 + 0.01);
        // Exactly enough room below still goes below.
        let tight = Rect::from_min_max(pos2(100.0, 800.0 - 4.0 - 200.0 - GAP - 56.0), pos2(900.0, 800.0 - 4.0 - 200.0 - GAP));
        assert_eq!(place(tight, 400.0, size, screen).y, tight.bottom() + GAP);
    }
}
