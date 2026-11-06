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
    NpVolumeHold,
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
    /// «Скорость прыжка» and «Скорость подсветки».
    JumpSpeed,
    LightSpeed,
    /// «Плавность» of the highlight.
    LyricsSmooth,
    LyricsDim,
    LyricsScale,
    LyricsLead,
    LyricsCascade,
    LyricsGlow,
    LyricsGap,
    LyricsGapWaves,
    /// «Высота волн» and «Размер индикатора» of the pause ring.
    LyricsGapHeight,
    LyricsGapSize,
    LyricsCountdown,
    /// «Подсвечивать»: by letters or by words, under every animation.
    LyricsUnit,
    /// «Мягкие края» of the lyrics and their strength.
    LyricsSoftEdges,
    LyricsSoftStrength,
    /// «Глубина мягких краёв» and «Затухание».
    LyricsSoftDepth,
    LyricsSoftFade,
    RailCompact,
    /// The parts of the side pane: «Очередь», «Текст песни», «Источник и
    /// качество».
    PaneQueue,
    PaneLyrics,
    PaneInfo,
    HomeCard,
    NpMotion,
    ReducedMotion,
    Tooltips,
    FpsLimit,
    // Основные.
    Autostart,
    AutostartHidden,
    /// «Быстрое открытие из трея».
    TrayKeepWindow,
    CoverClickPause,
    DblClickShape,
    TapWait,
    Keys,
    /// «Уведомления на экране»: how long a snackbar stays.
    ToastTime,
    SaveLyrics,
    LyricsReturn,
    LyricsCheck,
    /// «Подсветка по строкам для текстов без времени».
    LyricsLinesAuto,
    /// «Вид»: what the control under the lyrics opens.
    LyricsUi,
    /// «Подсветка по словам для песен, которые слушаю».
    LyricsWordsPlaying,
    LyricsWordsAll,
    LyricsRecognize,
    LyricsSeparate,
    LyricsDeep,
    LyricsGpu,
    LyricsModelsAuto,
    LyricsGenius,
    /// «Подсветка для песен без файла».
    LyricsStreams,
    /// What a song without a file keeps once its text is saved.
    LyricsStreamKeep,
    LyricsModels,
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
    /// «Предлагать MP3 вместо «фейкового» FLAC».
    FakeConvert,
    /// «Без вопроса».
    FakeAuto,
    /// «Вернуть FLAC из Корзины».
    FakeRestore,
    MusicBrainz,
    AcoustId,
    /// «Как к тебе обращаться».
    UserName,
    /// «Новое в радио и миксах».
    RadioNewShare,
    /// «Миксы только из скачанного».
    MixesDownloadedOnly,
    /// «Автовоспроизведение».
    Autoplay,
    /// «Продолжать мир после альбома».
    AutoplayWorld,
    /// Where radios pick from: «Миксы только из скачанного» and «Новое в
    /// радио» as one choice (the queue shows it; Settings has both rows).
    RadioSource,
    /// «Анализ звука»: how far it is (no control of its own).
    SoundAnalysis,
    /// «Не повторять сыгранное» in the radio and autoplay.
    RadioNoRepeat,
    /// The share of new music of one kind of radio.
    NewShareBy(v::RadioKind),
    /// «Миксы для тебя» with «С новым».
    MixesWithNew,
    /// «Фон меньше влияет на рекомендации».
    BackgroundCountsLess,
    /// «Делить профили по звуку».
    ProfilesSplit,
    /// «Названия профилей».
    ProfileNames,
    /// «Миксы профилей».
    MixesProfiles,
    /// «Всё вперемешку».
    MixesEverything,
    /// «Спокойное и погромче».
    MixesMoods,
    /// «На повторе».
    MixesOnRepeat,
    /// «Фон».
    BackgroundCards,
    /// «Нажатие на микс открывает его страницу».
    MixClickOpensList,
    /// «Кнопка «Не нравится»».
    DislikeButton,
    /// «Пропускать по дизлайку».
    DislikeSkips,
    /// «Пропускать и в своём».
    DislikeSkipEverywhere,
    /// «Учитывать в рекомендациях».
    DislikeInRecs,
    /// The disliked tracks, with «Снять».
    DislikedTracks,
    /// The artists not recommended, with «Снять».
    DislikedArtists,
    /// The mixes of «Миксы для тебя» left out, with «Вернуть».
    HiddenMixes,
    /// «Похожее по слушателям» (the graph of similar music).
    GraphUse,
    /// «Спрашивать YouTube Music и Deezer».
    GraphServices,
    /// «Спрашивать ListenBrainz и MusicBrainz».
    GraphOpenData,
    /// «Мои плейлисты и импорт истории».
    GraphPersonal,
    /// «Без истории — по самой библиотеке».
    LibraryAsTaste,
    /// «Ключ Last.fm».
    LastfmKey,
    // Источники.
    SpotifyEnabled,
    SpotifyPlayback,
    /// «Spotify Soloist»: Spotify's official player as the sound.
    SpotifySoloist,
    SpotifyPersonal,
    YouTubeEnabled,
    YouTubeCookies,
    DeezerEnabled,
    SoundCloudEnabled,
    /// «Вставить токен…» of SoundCloud.
    SoundCloudToken,
    LikeDownload,
    PushLikes,
    /// «Лайк альбома»: the album alone, or its tracks too.
    AlbumLike,
    /// «Лайк песни из сингла»: the song alone, or the single saved too.
    SingleLike,
    /// «Любимые альбомы»: a shelf of their own, or among the playlists.
    LikedAlbumsPlace,
    AutoDownloads,
    DownloadToasts,
    /// «Искать копии лучше»: how often, or never.
    UpgradeEvery,
    /// «Хорошее качество»: lossless only, or MP3 320 and up.
    UpgradeBar,
    /// «Замена файла»: asked in «Подтверди», or by itself.
    UpgradeAuto,
    /// One line of «Источники скачивания»: its switch and place.
    DownloadSource(v::SourceKey),
    PlaylistSync,
    ImportSpotify,
    ImportYouTube,
    ImportSoundCloud,
    ImportFiles,
    /// «Песни медиатеки YouTube Music» of an export.
    ImportSavedSongs,
    StreamCache,
    /// «Анализ звука стримов».
    StreamAnalysis,
    Prowlarr,
    /// «Soulseek»: where it runs and the account.
    Soulseek,
    SoulseekShare,
    SoulseekUpload,
    NoSeed,
    SeedLimit,
    SeedDays,
    SeedRatio,
    DownloadLimit,
    UploadLimit,
    Spotify,
    YouTube,
    SoundCloud,
    Nodes,
}

/// Where a radio of `kind` takes its tracks, in a few words (the line under
/// the queue's radio header, which opens the choices).
pub fn radio_source_line_of(p: &Prefs, kind: v::RadioKind) -> &'static str {
    match radio_source_of(p, kind) {
        0 => "Похожее из скачанного",
        1 => "Похожее из твоей библиотеки",
        _ => "Похожее из библиотеки и новое",
    }
}

/// Which of [`SettingId::RadioSource`]'s choices a radio of `kind` has:
/// its own share of new music decides between «Библиотека» and «С новым».
pub fn radio_source_of(p: &Prefs, kind: v::RadioKind) -> usize {
    if p.mixes_downloaded_only {
        0
    } else if p.new_share_of(kind) == v::RadioNewShare::None {
        1
    } else {
        2
    }
}

/// The changes choice `i` of [`SettingId::RadioSource`] makes for a radio
/// of `kind`: «Скачанное» is everyone's, the share of new music is the
/// kind's own (remembered for the next radio of it).
pub fn radio_source_patches(p: &Prefs, kind: v::RadioKind, i: usize) -> Vec<v::PrefPatch> {
    let mut out = vec![v::PrefPatch::MixesDownloadedOnly(i == 0)];
    match i {
        1 => out.push(v::PrefPatch::RadioNewShareBy(kind, v::RadioNewShare::None)),
        2 if p.new_share_of(kind) == v::RadioNewShare::None => {
            let share = if p.radio_new_share == v::RadioNewShare::None { v::RadioNewShare::Some } else { p.radio_new_share };
            out.push(v::PrefPatch::RadioNewShareBy(kind, share));
        }
        _ => {}
    }
    out
}

/// Settings tabs, in the order of the tab switch.
pub const TAB_GENERAL: usize = 0;
pub const TAB_RECS: usize = 1;
pub const TAB_SOURCES: usize = 2;
pub const TAB_APPEARANCE: usize = 3;
/// Labels of the tab switch, in [`SettingId::tab`] order.
pub const TABS: [&str; 4] = ["Основные", "Рекомендации", "Источники", "Внешний вид"];

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
    /// Which way the cover outline (false) or the ring (true) turns.
    Turn(bool),
    /// Lines following a scroll, with or without the cascade.
    Cascade,
    /// Lines rolling through the lyrics' soft edges (or cut, when off).
    Edges,
    /// The end of a pause: digits 3-2-1, or the ring simply fading away.
    Countdown,
}

/// One option of a choice row: its label (as on the button) and what it does.
#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub label: &'static str,
    pub help: &'static str,
}

/// A [`Choice`] (a literal, so the lists below are `'static`).
/// The system the player starts with and takes its theme from, named as
/// the user knows it.
const AUTOSTART_HELP: &str = if cfg!(windows) {
    "Плеер запускается сам, когда ты входишь в Windows. То же, что строка music.nikgob в «Автозагрузке» Диспетчера задач."
} else {
    "Плеер запускается сам, когда ты входишь в систему. То же, что music.nikgob в автозапуске рабочего стола."
};
const AUTOSTART_HIDDEN_HELP: &str = if cfg!(windows) {
    "При запуске вместе с Windows окно не открывается: плеер ждёт в трее, окно — по клику на значок. Обычный запуск открывает окно как всегда."
} else {
    "При запуске вместе с системой окно не открывается: плеер ждёт в трее, окно — по клику на значок. Обычный запуск открывает окно как всегда."
};
const SYSTEM_THEME_HELP: &str = if cfg!(windows) {
    "Как в настройках Windows: меняется вместе с системой."
} else {
    "Как в настройках системы: меняется вместе с ней."
};

macro_rules! c {
    ($label:expr, $help:expr) => {
        Choice { label: $label, help: $help }
    };
}

const OPUS: [u32; 4] = [128, 160, 192, 256];
/// «Уведомления на экране», seconds; 0 = until closed.
const TOAST_TIMES: [u8; 4] = [4, 6, 10, 0];
/// «Не повторять сыгранное», hours.
const NO_REPEAT: [u8; 5] = [1, 3, 6, 12, 24];
const SHARES: [v::RadioNewShare; 3] = [v::RadioNewShare::None, v::RadioNewShare::Some, v::RadioNewShare::More];

fn share_index(s: v::RadioNewShare) -> usize {
    SHARES.iter().position(|x| *x == s).unwrap_or(1)
}

impl SettingId {
    /// The tab the row lives on.
    pub fn tab(self) -> usize {
        use SettingId::*;
        match self {
            Design | Theme | Contrast | Scheme | ProgressWave | NpVolume | NpVolumeHold | CoverShape | ShapeRange | ShapePick
            | CoverSpin | CoverDir | CoverPulse | Ring | RingStyle | RingSpin | RingDir | RingBeat | Ambient
            | AmbientStrength | BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue
            | LyricsStyle | WaveFallback | WaveJump | JumpSpeed | LightSpeed | LyricsSmooth | LyricsDim | LyricsScale | LyricsLead | LyricsCascade | LyricsGlow
            | LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize | LyricsCountdown | LyricsUnit | LyricsSoftEdges | LyricsSoftStrength | LyricsSoftDepth | LyricsSoftFade | RailCompact | PaneQueue | PaneLyrics | PaneInfo | HomeCard | NpMotion
            | ReducedMotion | Tooltips | FpsLimit => {
                TAB_APPEARANCE
            }
            // Streaming, imports and torrents (docs/stage3.md §0 "Где настройки").
            Spotify | SpotifyEnabled | SpotifyPlayback | SpotifySoloist | SpotifyPersonal | YouTube | YouTubeEnabled | DeezerEnabled
            | SoundCloud | SoundCloudEnabled | SoundCloudToken | ImportSoundCloud
            | YouTubeCookies | LikeDownload | PushLikes | AlbumLike | SingleLike | LikedAlbumsPlace | AutoDownloads | DownloadToasts | UpgradeEvery | UpgradeBar | UpgradeAuto | DownloadSource(_) | PlaylistSync | ImportSpotify
            | ImportYouTube | ImportFiles | ImportSavedSongs | StreamCache | StreamAnalysis | Prowlarr | Soulseek | SoulseekShare | SoulseekUpload | NoSeed | SeedLimit | SeedDays
            | SeedRatio | DownloadLimit | UploadLimit => TAB_SOURCES,
            RadioNewShare | MixesDownloadedOnly | Autoplay | AutoplayWorld | RadioSource | SoundAnalysis | RadioNoRepeat | NewShareBy(_)
            | MixesWithNew | BackgroundCountsLess | ProfilesSplit | ProfileNames
            | MixesProfiles | MixesEverything | MixesMoods | MixesOnRepeat | BackgroundCards | MixClickOpensList | HiddenMixes
            | GraphUse | GraphServices | GraphOpenData | GraphPersonal | LibraryAsTaste | LastfmKey
            | DislikeButton | DislikeSkips | DislikeSkipEverywhere | DislikeInRecs | DislikedTracks | DislikedArtists => {
                TAB_RECS
            }
            _ => TAB_GENERAL,
        }
    }

    /// Not tested by users yet: the row carries a «Бета» mark and its card
    /// says so.
    pub fn beta(self) -> bool {
        use SettingId::*;
        matches!(self, ImportSpotify | ImportYouTube | ImportSoundCloud | ImportFiles | LyricsRecognize)
    }

    /// One line on what the option changes.
    pub fn help(self) -> &'static str {
        use SettingId::*;
        match self {
            Design => "Общий вид всего приложения. Пока доступен только Material 3, другие появятся позже.",
            Theme => "Светлый или тёмный фон приложения.",
            Contrast => "Насколько текст и кнопки выделяются на фоне. Выше — читать легче, но цвета резче.",
            Scheme => "Приложение окрашивается в цвета обложки играющего трека. Здесь выбирается, насколько ярко.",
            ProgressWave => "Пока играет музыка, полоска прогресса трека бежит волной. Выключи — будет ровной линией.",
            NpVolume => "Как менять громкость на экране трека: ползунок, всплывающая плашка или только кнопка.",
            NpVolumeHold => "Сколько плашка громкости остаётся на экране после того, как ты перестал крутить колесо или двигать ползунок.",
            CoverShape => "Обложка на экране трека может быть квадратной или вырезанной красивой фигурой.",
            ShapeRange => "Из этих фигур выбирается случайная для каждого трека. Нажми на фигуру, чтобы добавить или убрать её.",
            ShapePick => "Этой фигурой вырезается обложка, пока играет музыка.",
            CoverSpin => "Край обложки медленно поворачивается, а сама картинка остаётся на месте.",
            CoverDir => "В какую сторону поворачивается край обложки.",
            CoverPulse => "Обложка слегка увеличивается в такт музыке, как будто дышит.",
            Ring => "Вокруг обложки появляется кольцо из точек, которое шевелится под музыку.",
            RingStyle => "Как выглядит кольцо вокруг обложки.",
            RingSpin => "Кольцо из точек медленно вращается, пока играет музыка.",
            RingDir => "В какую сторону вращается кольцо.",
            RingBeat => "Точки кольца подскакивают на ударных и громких местах.",
            Ambient => "За экраном трека плывут мягкие цветные пятна в цветах обложки.",
            AmbientStrength => "Насколько заметны цветные пятна за экраном трека: от лёгкой дымки до яркого цвета.",
            BgShapes => "За экраном трека двигаются фигуры. Выбери, как они ведут себя.",
            BgCount => "Сколько фигур на фоне.",
            BgSize => "Насколько крупные фигуры на фоне.",
            BgOpacity => "Насколько фигуры видны: от еле заметных до ярких.",
            BgSpeed => "Как быстро фигуры плывут, крутятся и прыгают.",
            BgReact => "Насколько сильно фигуры отзываются на музыку. 0% — двигаются сами по себе.",
            BgColors => "В какие цвета окрашены фигуры на фоне.",
            BgHue => "Цвет фигур для варианта «Свой». Яркость подстраивается под тему, чтобы фон не спорил с интерфейсом.",
            LyricsStyle => "Как выделяется строка, которую сейчас поют.",
            WaveFallback => "У некоторых текстов известно только, когда начинается строка, а время слов нет. Что делать с такими текстами.",
            WaveJump => "Как высоко подпрыгивают буквы или слова в анимации «Волна». 0 — они только загораются.",
            JumpSpeed => "Как быстро буква или слово в «Волне» взлетает, когда его начинают петь, и встаёт на место. Больше — резче и короче.",
            LightSpeed => "Как быстро загорается буква или слово, когда его поют: от плавного появления до мгновенного. В караоке по буквам — насколько мягкий край у подсветки.",
            LyricsSmooth => "Подсветка идёт по времени букв и слогов. Чем плавнее, тем ровнее она скользит по строке, а в «Волне» соседние буквы поднимаются вместе с поющейся. 0% — строго по буквам.",
            LyricsDim => "Насколько бледнее остальные строки по сравнению с той, что поют сейчас.",
            LyricsScale => "Размер букв в тексте песни.",
            LyricsLead => "На сколько подсветка опережает голос. Глаз читает чуть раньше, чем слышит, поэтому подсветка, которая загорается немного заранее, кажется точной. Если она торопится — уменьшите, если отстаёт — увеличьте. Перемотка и время в тексте не меняются.",
            LyricsCascade => "При прокрутке дальние строки догоняют с небольшой задержкой — текст движется мягко, волной.",
            LyricsGlow => "Уже спетая часть строки слегка светится.",
            LyricsGap => "Когда в песне долго нет слов, между строками крутится кружок ожидания. Здесь выбирается его вид.",
            LyricsGapWaves => "Сколько волн на кружке ожидания. Больше — волны мельче и чаще.",
            LyricsGapHeight => "Насколько высокие волны на кружке ожидания.",
            LyricsGapSize => "Насколько большой кружок ожидания между строками.",
            LyricsCountdown => "За три секунды до новой строки кружок ожидания сменяется отсчётом 3, 2, 1.",
            LyricsUnit => "Что подсвечивается по очереди в любой анимации: каждая буква, каждый слог или каждое слово целиком. Слова через дефис подсвечиваются по частям.",
            LyricsSoftEdges => "Строки у верхнего и нижнего края текста расплываются и тают, а не обрезаются.",
            LyricsSoftStrength => "Насколько сильно расплывается строка у самого края. Размытие нарастает плавно: строка, которая подходит к краю, мягко теряет резкость, а появляющаяся постепенно становится чёткой.",
            LyricsSoftDepth => "Какая часть высоты текста сверху и снизу занята мягкими краями. Текущая строка всегда остаётся в чёткой середине.",
            LyricsSoftFade => "Как рано строка у края становится прозрачной: меньше — тает только у самого края, больше — начинает таять раньше.",
            RailCompact => "Левая панель всегда узкая: только значки разделов, без подписей и кнопки раскрытия.",
            PaneQueue => "Справа от страницы открывается очередь: кнопка в плеере внизу и Ctrl+Q. Выключено — у плеера больше места под полосу перемотки.",
            PaneLyrics => "Справа от страницы открывается текст песни: кнопка в плеере внизу и пункт в меню трека. На экране трека текст есть всегда.",
            PaneInfo => "Справа от страницы открываются источник и качество трека: пункт «Источник и качество» в меню трека. На экране трека они есть всегда.",
            HomeCard => "Большая карточка вверху Главной: альбом или серия, которую слушаешь, и рядом другие твои миры — что дослушать и что ещё не начато. Без неё Главная начинается сразу с приветствия.",
            NpMotion => "Как экран трека открывается из плеера внизу и закрывается обратно.",
            ReducedMotion => "Убирает подпрыгивания, вращения и волны во всём приложении.",
            Tooltips => "Короткие подсказки, когда задерживаешь курсор на кнопке. Карточки с примерами в настройках показываются всегда.",
            FpsLimit => "Как часто перерисовывается экран. Меньше — тише вентилятор и дольше работает батарея.",
            Autostart => AUTOSTART_HELP,
            TrayKeepWindow => "Крестик прячет окно в трей, но не закрывает его совсем: по клику на значок оно появляется сразу, там же, где было. Пока окно спрятано, оно ничего не рисует и не тратит процессор, но держит в памяти свои картинки и шрифты. Выключи — окно будет закрываться полностью и собираться заново при открытии: памяти меньше, открывается чуть дольше.",
            AutostartHidden => AUTOSTART_HIDDEN_HELP,
            CoverClickPause => "Нажатие на обложку на экране трека ставит музыку на паузу и снова запускает.",
            DblClickShape => "Двойной клик по экрану трека меняет фигуру обложки.",
            TapWait => "Чтобы отличить одиночный клик (пауза) от двойного (другая фигура), плеер чуть ждёт второго клика. Меньше — пауза срабатывает быстрее, но двойной клик нужно делать быстрее. «Сразу» — пауза мгновенная, а второй клик снимает её обратно и меняет фигуру.",
            Keys => "Клавиши, которые работают на экране трека.",
            ToastTime => "Сколько держится уведомление внизу окна, пока не уйдёт само. Пока на нём курсор, время стоит.",
            SaveLyrics => "Найденный в интернете текст песни сохраняется прямо в файл трека и потом показывается без интернета.",
            LyricsReturn => "Если ты сам прокрутил текст, через столько секунд он вернётся к строке, которую поют.",
            LyricsCheck => "Плеер слушает голос играющей песни и проверяет, её ли это текст. Текст другой песни или другой версии уступает подходящему, а строки, которые спешат или опаздывают, встают по голосу.",
            LyricsUi => "Источник текста и то, как его видеть (по словам, по строкам или без подсветки), всегда выбираются в маленьком меню у кнопки под текстом. Здесь выбирается, где делать подсветку по строкам или словам: прямо в этом меню, в окне поверх плеера или на отдельном экране «Текст песни».",
            LyricsLinesAuto => "Если ни у одного источника нет текста со временем строк, плеер сам ставит строки простого текста по голосу играющей песни, и текст идёт вместе с песней.",
            LyricsWordsPlaying => "Плеер сам расставляет слова по голосу в песне, которая играет, и подсветка идёт по словам даже там, где источник дал только строки или простой текст.",
            LyricsWordsAll => "То же для всех песен с файлом, понемногу в фоне, пока плеер открыт. На большой библиотеке это часы работы видеокарты. Сделанное хранится в библиотеке и не делается заново.",
            LyricsRecognize => "Если текста нет нигде, плеер пробует расслышать слова сам. Работает медленно и ошибается, особенно в быстром рэпе и сильно обработанном голосе.",
            LyricsSeparate => "Перед сверкой голос отделяется от музыки: слова ложатся точнее. На процессоре это долго, около двух минут на песню.",
            LyricsDeep => "Подсветка делается моделями крупнее: голос отделяется чище, слова слушаются дважды. Модели скачиваются при первом включении; на видеокарте песня занимает до минуты, на процессоре — несколько минут.",
            LyricsGpu => "Где считают модели. На видеокарте (через DirectML) в десятки раз быстрее; если она не справится, плеер сам перейдёт на процессор.",
            LyricsModelsAuto => "Нужные модели скачиваются сами, когда понадобятся. Выключено — только кнопкой ниже.",
            LyricsGenius => "Если подходящего текста со временем нет, плеер берёт простой текст с genius.com. Найденный текст хранится в библиотеке, второй раз его не ищут.",
            LyricsStreams => "Подсветку по словам можно сделать и для песни, которая играет с YouTube: плеер на время скачивает её звук, слушает голос и удаляет звук. Только для песен, где ясно, кто и что поёт.",
            LyricsStreamKeep => "Что «Сохранить текст» кладёт в папку исполнителя в библиотеке для песни без файла.",
            LyricsModels => "Модели, которыми плеер слушает голос и ставит строки и слова. Хранятся рядом с библиотекой; для песни скачиваются только нужные.",
            OutputDevice => "Через что играет звук: колонки, наушники и т. д. Переключается без паузы.",
            Gapless => "Треки альбома идут друг за другом без тишины, как на диске.",
            Crossfade => "Конец трека плавно затихает, пока начинается следующий.",
            Normalization => "Все треки звучат примерно одинаково громко, не нужно крутить громкость.",
            LibraryDir => "Папка на диске, где лежит твоя музыка.",
            MastersDir => "Сюда откладываются исходные файлы FLAC после сжатия в Opus, чтобы ничего не потерять.",
            Inbox => "Положи файлы в эту папку — и они сами добавятся в библиотеку.",
            NamingRules => "Как называются папки и файлы после добавления в библиотеку.",
            Trash => "Включена — удалённое 30 дней лежит в Библиотека → Корзина, и его можно вернуть. Выключена — удаляется сразу.",
            ExportData => "Лайки и плейлисты хранятся в обычных файлах рядом с музыкой, их легко сохранить.",
            OpusBitrate => "Качество звука при сжатии новых треков. Выше — звук лучше, но файлы больше.",
            FakeFlac => "Находит «ненастоящий» FLAC — файлы, сделанные из MP3: высокое качество в них только на бумаге.",
            FakeConvert => "Явный «ненастоящий» FLAC (MP3 внутри, резкий срез) появляется в «Подтверди»: можно превратить его в MP3 320 (или V0, если исходник был слабее) — звук тот же, места меньше — или оставить. Теги, обложка, текст и всё о треке остаются, FLAC уходит в Корзину на 30 дней. Выключено — файл остаётся как есть, подписан тем, что он на самом деле. Нужен ffmpeg.",
            FakeAuto => "Явный «ненастоящий» FLAC сразу становится MP3, без вопроса в «Подтверди». FLAC всё равно 30 дней лежит в Корзине.",
            FakeRestore => "Возвращает из Корзины все FLAC, которые стали MP3, и снова играет их; MP3 уходят в Корзину. Лайки, прослушивания, плейлисты и тексты остаются.",
            MusicBrainz => "Почта или сайт для сервиса MusicBrainz, откуда берутся названия и обложки. Сервис просит его указать.",
            AcoustId => "Ключ сервиса AcoustID: он узнаёт трек по звуку, если у файла нет названий.",
            UserName => "Имя в приветствии на главной: «Добрый вечер, Ник». Пустое поле — приветствие без имени.",
            Prowlarr => "Адрес и ключ программы Prowlarr — через неё ищутся раздачи с музыкой.",
            Soulseek => "Свой аккаунт в сети Soulseek: новый ник и пароль заводятся сами при первом входе. «В плеере» — клиент работает внутри плеера в фоне; «На сервере» — утилита nikgob-slsk на сервере или Raspberry Pi, плеер забирает у неё скачанное. Хранится в settings.toml.",
            SoulseekShare => "Другие люди в Soulseek могут скачивать у тебя треки из папки библиотеки (только чтение). Сеть держится на обмене: кто раздаёт, тому охотнее отдают.",
            SoulseekUpload => "Максимальная скорость, с которой Soulseek отдаёт треки другим.",
            NoSeed => "Скачанные треки сразу перестают раздаваться другим.",
            SeedLimit => "Сколько всего можно отдать другим, после чего раздача остановится.",
            SeedDays => "Сколько дней раздавать скачанное, потом раздача удаляется.",
            SeedRatio => "Раздача остановится, когда ты отдашь во столько раз больше, чем скачал.",
            DownloadLimit => "Максимальная скорость скачивания, чтобы не занимать весь интернет.",
            UploadLimit => "Максимальная скорость раздачи другим.",
            Spotify => "Вход в аккаунт Spotify через браузер со своим ключом: поиск, лайки, плейлисты и история. Нужен Premium — без него Spotify не подключить, а YouTube Music работает и так. Пароль приложение не видит.",
            SpotifyEnabled => "Выключи — Spotify не используется нигде: ни в поиске, ни для звука, ни для лайков. Вход при этом не забывается.",
            SpotifyPlayback => "Отдельное разрешение на звук из Spotify (второе окно согласия). Пока его нет, те же треки играют с YouTube Music — в плеере это видно по подписи «YouTube».",
            SpotifySoloist => "Spotify Soloist — официальный плеер Spotify без окна. Скачанный, со своим ключом и выбранный один раз в приложении Spotify, он играет треки Spotify; без него они играют как раньше. Скачивается только с сервера Spotify и только с твоего согласия. Ключ у каждого свой, на developer.spotify.com/dashboard/soloist, нужен Premium. Кроссфейд и «без пауз» к его трекам не применяются.",
            SpotifyPersonal => "Ключ — это Client ID своего приложения на developer.spotify.com, со своей квотой запросов. Spotify пускает такое приложение, только пока у владельца Premium. Заводится один раз; хранится в settings.toml.",
            YouTube => "Вход нужен только для лайков, плейлистов и истории; поиск и звук работают и без него. «Войти через Google» открывает окно со страницей входа Google: пароль вводится только там, приложение его не видит и берёт лишь cookies сессии. Окно запоминает вход, так что повторный вход — в один клик; «Выйти» забывает и его. Если Google не пускает во встроенном окне — «Вставить cookies…».",
            YouTubeEnabled => "Выключи — YouTube Music не используется: ни в поиске, ни как запасной звук для треков Spotify.",
            DeezerEnabled => "Deezer — открытый каталог: поиск, альбомы и дискографии исполнителей без аккаунта и без ключа. Своего звука у него нет: трек Deezer играет с YouTube Music (по ISRC, иначе по исполнителю, названию и длительности). Лайк трека Deezer остаётся здесь — в Deezer ничего не уходит. Выключи — поиск и дискографии обойдутся YouTube Music и Spotify.",
            SoundCloud => "Вход нужен для лайков, плейлистов, истории и подписок; поиск и звук работают и без него. «Войти» открывает страницу входа SoundCloud в окне приложения: пароль вводится только там, приложение берёт лишь токен сессии и хранит его в системном хранилище паролей. Треки, которые SoundCloud отдаёт только в защищённом виде или только 30-секундным отрывком, играют с YouTube Music. В России soundcloud.com открывается только через VPN.",
            SoundCloudEnabled => "Выключи — SoundCloud не используется: ни в поиске, ни для звука, ни для лайков. Вход при этом не забывается.",
            SoundCloudToken => "Запасной вход: на soundcloud.com, пока ты вошёл, открой инструменты разработчика → Application → Cookies и скопируй значение oauth_token. Подойдут и cookies.txt, и заголовок Cookie.",
            ImportSoundCloud => "Забирает из SoundCloud свои и понравившиеся плейлисты и до 1000 последних прослушиваний. Лайки приходят сами после входа; подписки и репосты учат рекомендации.",
            YouTubeCookies => "Запасной вход, если окно «Войти через Google» не помогло: выгрузи cookies.txt для music.youtube.com расширением браузера, пока ты вошёл, и вставь сюда. Обычно хватает «Войти через Google».",
            LikeDownload => "Что скачивать, когда лайкаешь трек из стрима: только его (вырезается из раздачи альбома), сразу весь альбом или ничего — тогда лайк остаётся только лайком. Лайк альбома качает весь альбом. «Скачать» у трека или альбома качает всегда, без лайка.",
            PushLikes => "Лайк здесь ставится и в Spotify / YouTube Music, а снятый лайк снимается и там. Только для треков, которые есть в сервисе.",
            AlbumLike => "Что делает сердечко у альбома: только кладёт альбом в «Любимые альбомы» или ещё и лайкает все его треки — тогда они попадают в «Любимые».",
            SingleLike => "Сингл — это альбом из одной песни. Лайк такой песни может заодно класть сингл в «Любимые альбомы».",
            LikedAlbumsPlace => "Где в библиотеке лежат любимые альбомы: отдельной полкой или вперемешку с плейлистами.",
            AutoDownloads => "Сколько треков из «Хочу скачать» ищутся и качаются одновременно сами. Меньше — тише для сети.",
            DownloadToasts => "Всплывающие сообщения о загрузках, которые идут сами: раздача не нашлась, начала качаться, скачалась. Выключено — всё это видно только в «Загрузках» и «Хочу скачать».",
            UpgradeEvery => "Треки и альбомы, у которых файл хуже выбранного в «Хорошее качество», плеер сам ищет снова во всех источниках скачивания — по нескольку в час, чтобы не мешать. Что нашлось лучше, заменяет файл: лайки, прослушивания, плейлисты и текст остаются, старый файл уходит в корзину. Ход поиска видно в «Хочу скачать».",
            UpgradeBar => "Какое качество считать достаточным. Файлы хуже ищутся снова, и заменяет их только копия не хуже этого.",
            UpgradeAuto => "Найденная копия может оказаться другим изданием (ремастер, другой трек-лист), поэтому по умолчанию замена ждёт тебя в «Подтверди». «Заменять сам» — плеер скачивает и заменяет без вопросов.",
            DownloadSource(v::SourceKey::Torrents) => "Раздачи, которые находит Prowlarr (адрес и ключ — ниже, в «Торренты»). Из альбома или дискографии берётся только нужный трек. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Khinsider) => "Саундтреки игр с downloads.khinsider.com, часто во FLAC. Спрашивается только для саундтреков: альбом так и называется или песня написана для игры. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Soulseek) => "Отдельные треки от других людей в сети Soulseek, часто во FLAC. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Qobuz) => "Каталог Qobuz во FLAC (до Hi-Res) через открытый сервис-посредник, без своего аккаунта. Такие сервисы то работают, то нет: если не отвечает, плеер просто попробует позже. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Tidal) => "Каталог Tidal во FLAC через открытые сервисы-посредники, без своего аккаунта. Их часто блокируют: если не отвечают, плеер попробует позже. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            PlaylistSync => "Как новый плейлист, связанный с сервисом, следит за ним. У каждого плейлиста это можно поменять в его меню ⋮.",
            ImportSpotify => "Забирает из Spotify свои плейлисты и недавние прослушивания. Лайки приходят сами сразу после входа. Плейлисты появятся в библиотеке; скачивается только то, что скачаешь сам.",
            ImportYouTube => "То же из YouTube Music: плейлисты и история. Лайки приходят сами после входа.",
            RadioNewShare => "Радио по треку или исполнителю и бесконечные миксы (настроения, «Твои миксы», «Новое для тебя») подмешивают музыку, которой у тебя ещё нет: похожее из YouTube Music и Deezer. Здесь — сколько. «Твой микс» на главной собирается только из твоего. Пока включены «Миксы только из скачанного», не действует: нового нет нигде.",
            Autoplay => "Когда в очереди играет последний трек, она не кончается: следом сами добавляются похожие треки, как в радио по этому треку, и дальше дописываются по мере игры. Повтор не мешает: при включённом повторе очередь просто начинается заново. Откуда берутся треки — как у радио: только скачанное, вся библиотека или с новым из сервисов (это же переключается прямо в очереди).",
            AutoplayWorld => "Если в очереди был альбом, после него автовоспроизведение сначала играет следующие треки его мира по порядку альбомов — саундтрека франшизы или дискографии исполнителя, что ещё не слышал, — и только когда мир кончится, похожее, как радио. Выключи — сразу похожее.",
            RadioSource => "Откуда радио, бесконечные миксы и автовоспроизведение берут треки. Это те же две настройки, что ниже в «Рекомендациях»: «Миксы только из скачанного» и «Новое в радио и миксах».",
            MixesDownloadedOnly => "Всё, что подбирается для тебя — «Твой микс», дейлист, плитки главной, настроения, «Твои миксы», миксы исполнителей и радио, — берёт только треки, у которых есть файл: ни стримов, ни новой музыки из сервисов. Плитки, которым не хватило таких треков, пропадают, «Ещё не скачано» не показывается. Главная пересобирается сразу. «Новое для тебя» остаётся: это отдельная полка новинок.",
            SoundAnalysis => "Миксы и радио подбирают треки по звуку: темп, громкость, яркость, ровность. Для этого каждый трек один раз анализируется в фоне — тихо, с пониженным приоритетом. Пока анализ идёт, подборки уже работают, просто чуть менее точно.",
            StreamAnalysis => "Лайкнутые и плейлистные треки без файла тоже анализируются: их звук по одному тихо скачивается с YouTube Music в память (несколько мегабайт), анализируется и сразу выбрасывается. Не чаще раза в 20 секунд и не больше 300 треков в день; трек, сыгравший из стрима, анализируется из кэша без лишнего скачивания. Выключи — стримы останутся без анализа, а миксы будут подбирать их по исполнителю и истории.",
            StreamCache => "Недавно игравшие треки хранятся на диске, и «Назад» или повтор не качают их заново. YouTube — файлы как есть, Spotify — только в зашифрованном виде, как у официального клиента. Лимит у каждого сервиса свой; старое удаляется само.",
            ImportFiles => "Файлы выгрузки — вся история прослушиваний за годы. Spotify: Аккаунт → Конфиденциальность → «Расширенная история». Google Takeout: YouTube и YouTube Music. История учит рекомендации и не добавляет в библиотеку ни треков, ни альбомов; само ничего не скачивается.",
            ImportSavedSongs => "Песни, сохранённые в медиатеке YouTube Music (в Takeout — файл «music library songs»), учат рекомендации, что тебе нравится: без лайков, без загрузок, в «Библиотеке» и «Любимых» их не будет. Выключи — этот файл при импорте пропускается.",
            Nodes => "Синхронизация и резервная копия на другие устройства — появится позже.",
            NewShareBy(v::RadioKind::Track) => "Сколько нового в радио по треку. Выбор в очереди над радио меняет это же.",
            NewShareBy(v::RadioKind::Artist) => "Сколько нового в радио по исполнителю.",
            NewShareBy(v::RadioKind::Mix) => "Сколько нового в бесконечных миксах: настроения, «Твои миксы», «Новое для тебя».",
            NewShareBy(v::RadioKind::Autoplay) => "Сколько нового в автовоспроизведении, когда очередь кончилась.",
            MixesWithNew => "«Миксы для тебя» на главной: в каждом миксе немного новой музыки его вкуса. Сколько — по тому, как новое у тебя приживается.",
            BackgroundCountsLess => "Музыка, что играла фоном — ты не переключал треки, а окно было свёрнуто или закрыто другим, — меньше меняет баланс твоих вкусов на главной. Выключи — фон считается наравне со всем.",
            ProfilesSplit => "Песни одного вида, но очень разные по звуку — например, спокойные и громкие японские — становятся двумя профилями вкуса. Выключи — один профиль на вид песен.",
            ProfileNames => "Как называются профили твоего вкуса.",
            MixesProfiles => "Микс каждого профиля твоего вкуса: «Русские песни», «Саундтреки без слов».",
            MixesEverything => "Все твои профили в одном миксе, каждый своей долей, с плавными переходами.",
            MixesMoods => "Самое спокойное и самое громкое из того, что ты любишь, на любом языке.",
            MixesOnRepeat => "Что ты включал чаще всего за последние недели.",
            RadioNoRepeat => "Радио и автовоспроизведение не ставят то, что уже играло за это время или в этом сеансе, пока есть что-то ещё.",
            MixClickOpensList => "Нажатие на карточку микса открывает его страницу с песнями, а играет он по кнопке ▶. Выключи — микс будет играть сразу по нажатию на карточку, а страница откроется из ⋮ → «Список треков».",
            BackgroundCards => "Ровная музыка без слов для фона. Появляется, только если ты так слушаешь.",
            DislikeButton => "Кнопка рядом
 с ♡ на экране трека. «Не нравится» остаётся и в меню ⋮ любого трека.",
            DislikeSkips => "Трек, которому поставили «Не нравится», сразу пропускается.",
            DislikeSkipEverywhere => "Треки с «Не нравится» и исполнители, которых не рекомендовать, пропускаются и в твоих альбомах и плейлистах. Выключено — только в миксах, радио и автовоспроизведении; трек, выбранный тобой, играет всегда.",
            DislikeInRecs => "«Не нравится» чуть опускает и самые похожие по звуку треки. Выключено — уходит только сам трек. Исполнителя дизлайк не трогает.",
            DislikedTracks => "Треки с «Не нравится». «Снять» возвращает трек в рекомендации.",
            DislikedArtists => "Исполнители, которых не рекомендовать: их нет в миксах, радио и новом. «Снять» возвращает.",
            HiddenMixes => "Миксы, убранные с главной через ⋮ → «Не предлагать этот микс». «Вернуть» снова показывает микс.",
            GraphUse => "Радио, миксы и новое учитывают, что другие люди слушают вместе: радио YouTube Music, похожих исполнителей Deezer, ListenBrainz и Last.fm. Так рекомендации работают и без истории, по одним твоим файлам. Выключи — остаются звук, твоя история и тип песни.",
            GraphServices => "Понемногу и в фоне спрашивать у YouTube Music радио твоих треков, а у YouTube Music и Deezer — похожих исполнителей. Ответы хранятся у тебя и работают без сети.",
            GraphOpenData => "Понемногу и в фоне спрашивать открытые данные: похожих исполнителей и треки в ListenBrainz, жанры исполнителей в MusicBrainz. Без ключей и аккаунтов.",
            GraphPersonal => "Треки, которые стоят рядом в твоих плейлистах или шли подряд в загруженной истории YouTube Music или Spotify, считаются похожими.",
            LibraryAsTaste => "Пока истории мало, вкус берётся из самой библиотеки: профили и миксы есть с первого запуска, а треки, вокруг которых собрана коллекция, стоят выше. Чем больше ты слушаешь, тем меньше это значит.",
            LastfmKey => "Свой бесплатный ключ Last.fm API: с ним похожие треки и исполнители спрашиваются и у Last.fm. Хранится в settings.toml. Пусто — Last.fm не спрашивается.",
        }
    }

    /// The options of a choice row (segmented buttons or a menu), in order;
    /// empty for every other row.
    pub fn choices(self) -> &'static [Choice] {
        match self {
            Self::Theme => &[
                c!("Светлая", "Белый фон и тёмный текст. Удобно днём."),
                c!("Тёмная", "Тёмный фон. Меньше слепит вечером, обложки смотрятся ярче."),
                c!("Системная", SYSTEM_THEME_HELP),
            ],
            Self::Contrast => &[
                c!("Обычный", "Стандартные мягкие цвета."),
                c!("Средний", "Текст и края кнопок заметнее."),
                c!("Высокий", "Самый чёткий текст, цвета жёстче."),
            ],
            Self::Scheme => &[
                c!("Как обложка", "Цвета почти как на обложке."),
                c!("Спокойная", "Мягкие, приглушённые цвета из обложки."),
                c!("Яркая", "Насыщенные, сочные цвета из обложки."),
                c!("Смелая", "Неожиданные сочетания: оттенки уходят в сторону от обложки."),
                c!("Точная", "Цвета точь-в-точь как на обложке, даже у кнопок."),
                c!("Чёрно-белая", "Только чёрный, белый и серый, без цвета."),
                c!("Сдержанная", "Почти серые цвета с лёгким оттенком обложки."),
            ],
            Self::NpVolume => &[
                c!("Всегда", "Ползунок всегда рядом с кнопкой звука."),
                c!("При изменении", "Внизу по центру всплывает плашка, пока меняешь громкость, и прячется через пару секунд."),
                c!("Нет", "Только кнопка звука; громкость — колесом мыши."),
            ],
            Self::CoverShape => &[
                c!("Квадрат", "Обложка как есть, обычный квадрат."),
                c!("Фигура", "Обложка вырезана одной фигурой, которую ты выберешь ниже."),
                c!("Случайная", "У каждого трека своя случайная фигура."),
            ],
            Self::LyricsUi => &[
                c!("Меню", "Всё делается прямо из меню у кнопки: подсветка, вид текста, сохранение."),
                c!("Окно", "В меню у кнопки только источники; пункт «Подсветка…» открывает окно поверх плеера: весь текст, подсветка и сохранение."),
                c!("Экран", "В меню у кнопки только источники; пункт «Подсветка…» открывает отдельный экран «Текст песни», как настройки."),
            ],
            Self::LyricsStreamKeep => &[
                c!("Только текст", "Рядом с остальной музыкой исполнителя ложится только файл текста; песня играет с YouTube, как раньше."),
                c!("Песню и текст", "Песня скачивается с YouTube в библиотеку как файл, текст ложится рядом с ней."),
            ],
            Self::LyricsGpu => &[
                c!("Видеокарта", "Быстро: несколько секунд на песню."),
                c!("Процессор", "Медленно: пара минут на песню, зато видеокарта свободна."),
            ],
            Self::CoverDir | Self::RingDir => &[
                c!("По часовой", "Поворачивается как стрелки часов: 12 → 3 → 6."),
                c!("Против", "Поворачивается против часовой стрелки: 12 → 9 → 6."),
            ],
            Self::RingStyle => &[
                c!("Круг", "Точки стоят ровным кругом вокруг обложки."),
                c!("Контур", "Точки повторяют форму обложки."),
                c!("Бары", "Вокруг обложки столбики, которые растут под музыку, как эквалайзер."),
            ],
            Self::Ambient => &[
                c!("Выкл", "Обычный ровный фон."),
                c!("Свечение", "Цветные пятна медленно плывут за экраном."),
                c!("Под звук", "Пятна плывут и вспыхивают на басах."),
            ],
            Self::BgShapes => &[
                c!("Выкл", "Фон без фигур."),
                c!("Прыгают", "Фигуры лежат внизу экрана и подпрыгивают в такт ударным."),
                c!("Парят", "Фигуры медленно плавают по экрану и светятся в такт."),
                c!("Меняют форму", "Фигуры плывут и на ударах превращаются в другие."),
            ],
            Self::BgColors => &[
                c!("Тема", "Мягкие светлые цвета приложения — спокойно и ненавязчиво."),
                c!("Акцент", "Все фигуры одного главного цвета."),
                c!("Яркие", "Фигуры трёх ярких цветов приложения."),
                c!("Свой", "Цвет, который ты выберешь в строке «Свой цвет»."),
            ],
            Self::BgHue => &[
                c!("Красный", "Фигуры в красных тонах."),
                c!("Оранжевый", "Фигуры в оранжевых тонах."),
                c!("Жёлтый", "Фигуры в жёлтых тонах."),
                c!("Зелёный", "Фигуры в зелёных тонах."),
                c!("Бирюзовый", "Фигуры в бирюзовых тонах."),
                c!("Синий", "Фигуры в синих тонах."),
                c!("Фиолетовый", "Фигуры в фиолетовых тонах."),
                c!("Розовый", "Фигуры в розовых тонах."),
            ],
            Self::LyricsStyle => &[
                c!("Караоке", "Строка закрашивается слева направо, как в караоке: буква за буквой или слово за словом."),
                c!("Фокус", "Строка, которую поют, становится яркой: целиком или слово за словом, когда его начинают петь."),
                c!("Волна", "Буквы или слова по очереди загораются и подпрыгивают."),
            ],
            Self::LyricsUnit => &[
                c!("По буквам", "Караоке закрашивает слова буква за буквой, в «Волне» прыгает каждая буква, «Фокус» зажигает строку целиком."),
                c!("По слогам", "Каждый слог заливается, пока его поют: быстрые слоги пробегают быстро, протяжный светится всю ноту. В «Волне» слово поднимается со своими слогами."),
                c!("По словам", "Каждое слово загорается целиком, когда его начинают петь, а в «Волне» подпрыгивает всё слово."),
            ],
            Self::WaveFallback => &[
                c!("Угадывать слова", "Время слов угадывается по строке, и выбранная анимация идёт по ним как обычно."),
                c!("Спокойный Фокус", "Слова не идут по очереди, строка просто становится яркой целиком."),
            ],
            Self::LyricsGap => &[
                c!("Линия", "Тонкий ровный кружок."),
                c!("Волна", "Тонкий волнистый кружок."),
                c!("Толстая линия", "Толстый ровный кружок."),
                c!("Толстая волна", "Толстый волнистый кружок."),
            ],
            Self::NpMotion => &[
                c!("Плавно", "Экран открывается мягко и неторопливо."),
                c!("Живо", "Быстро и с лёгким пружинящим толчком в конце."),
                c!("Быстро", "Почти мгновенно, без толчка."),
            ],
            Self::ToastTime => &[
                c!("4 с", "Коротко: уведомление уходит быстро."),
                c!("6 с", "Чуть дольше, чтобы успеть прочитать."),
                c!("10 с", "Долго: есть время дочитать и нажать «Отменить»."),
                c!("Пока не закрою", "Уведомление висит, пока не закроешь его крестиком."),
            ],
            Self::DblClickShape => &[
                c!("Выкл", "Двойной клик ничего не меняет."),
                c!("Только в режиме «Случайная»", "Новая случайная фигура, только если выбрана «Случайная»."),
                c!("Всегда", "Всегда; выбранная фигура сменяется следующей."),
            ],
            Self::LikeDownload => &[
                c!("Только трек", "Качается один трек: из раздачи альбома берётся только его файл."),
                c!("Весь альбом", "Лайк трека скачивает весь альбом, в котором он вышел."),
                c!("Ничего", "Лайк — только лайк: ничего не качается. «Скачать» у трека и альбома качает и так."),
            ],
            Self::AlbumLike => &[
                c!("Только альбом", "Альбом попадает в «Любимые альбомы», его треки не трогаются."),
                c!("Альбом и все треки", "Все треки альбома лайкаются: попадают в «Любимые» и, если включено, в сервисы. Снять лайк с альбома — снять со всех."),
            ],
            Self::SingleLike => &[
                c!("Только песня", "Лайк песни — только лайк песни."),
                c!("И сам сингл", "Лайк песни из сингла кладёт сингл в «Любимые альбомы», снятый лайк убирает."),
            ],
            Self::LikedAlbumsPlace => &[
                c!("Отдельно", "Своя полка «Любимые альбомы» в библиотеке."),
                c!("С плейлистами", "Альбомы стоят среди плейлистов, по времени лайка."),
            ],
            Self::UpgradeEvery => &[
                c!("Раз в день", "Каждый файл хуже нужного проверяется раз в сутки."),
                c!("Раз в неделю", "Раз в неделю — обычно этого хватает: новые раздачи и релизы появляются не каждый день."),
                c!("Раз в месяц", "Реже всего: меньше запросов к источникам."),
                c!("Никогда", "Файлы не ищутся снова; то, что уже ищется, убирается из «Хочу скачать»."),
            ],
            Self::UpgradeBar => &[
                c!("Только lossless", "Ищется FLAC (или другой lossless) для всего, что в MP3, AAC или Opus."),
                c!("MP3 320 и выше", "Ищется копия не хуже MP3 320 для файлов хуже него, например MP3 128."),
            ],
            Self::UpgradeAuto => &[
                c!("Спрашивать", "Найденное лучшее качество ждёт в «Подтверди»: видно, что на что меняется, например «FLAC вместо MP3 128»."),
                c!("Заменять сам", "Лучшая копия скачивается и заменяет файл без вопросов."),
            ],
            Self::PlaylistSync => &[
                c!("Двусторонняя", "Правки здесь уходят в сервис, правки там приходят сюда."),
                c!("Только слежка", "Плейлист повторяет сервис; свои правки здесь туда не уходят."),
                c!("Разово", "Снимок: плейлист скопирован один раз и дальше живёт сам по себе."),
            ],
            Self::RadioSource => &[
                c!("Скачанное", "Только треки с файлом: без стримов и без новой музыки. Это же включает «Миксы только из скачанного» на главной."),
                c!("Библиотека", "Вся твоя музыка: скачанное и лайкнутое или из плейлистов, что пока играет из стрима. Без нового."),
                c!("С новым", "Твоя музыка и похожая новая из YouTube Music и Deezer (сколько — в «Новое в радио и миксах»)."),
            ],
            Self::RadioNewShare | Self::NewShareBy(_) => &[
                c!("Без нового", "Только твоя музыка: что лайкнуто, в плейлистах и в библиотеке."),
                c!("Немного", "Примерно каждый пятый трек — новый для тебя, похожий на то, что играет."),
                c!("Побольше", "Примерно два трека из пяти — новые: чтобы чаще находить незнакомое."),
            ],
            Self::BackgroundCards => &[
                c!("Не показывать", "Миксов «Фон» на главной нет."),
                c!("Один", "Один микс «Фон»."),
                c!("Два", "До двух: потише и поживее."),
                c!("До трёх", "Сколько подходит тому, как ты слушаешь фоном, но не больше трёх."),
            ],
            Self::RadioNoRepeat => &[
                c!("Час", "Сыгранное может вернуться через час."),
                c!("3 часа", "Сыгранное может вернуться через три часа."),
                c!("6 часов", "Сыгранное может вернуться через шесть часов."),
                c!("12 часов", "Сыгранное может вернуться через полдня."),
                c!("Сутки", "Сыгранное может вернуться на следующий день."),
            ],
            Self::ProfileNames => &[
                c!("Тип и язык", "«Японские песни», «Саундтреки без слов»; исполнители — под названием."),
                c!("Исполнители", "Названия из главных исполнителей профиля."),
            ],
            Self::OpusBitrate => &[
                c!("128", "Файлы поменьше; на слух почти как оригинал."),
                c!("160", "Хороший звук при разумном размере файлов."),
                c!("192", "Звук с запасом для хороших наушников."),
                c!("256", "Лучший звук; файлы вдвое больше, чем при 128."),
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
            Self::LyricsGpu => usize::from(!p.lyrics_gpu),
            Self::LyricsStreamKeep => usize::from(p.lyrics_stream_keep.min(1)),
            Self::LyricsUi => usize::from(p.lyrics_ui.min(2)),
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
            Self::BgHue => usize::from(p.bg_shapes_hue).min(7),
            Self::LyricsStyle => usize::from(p.lyrics_style).min(2),
            Self::LyricsUnit => usize::from(p.lyrics_unit.min(2)),
            Self::WaveFallback => usize::from(p.wave_syllables_only),
            Self::LyricsGap => usize::from(p.lyrics_gap_style).min(3),
            Self::NpMotion => usize::from(p.np_motion).min(2),
            Self::DblClickShape => usize::from(p.dbl_click_shape).min(2),
            Self::ToastTime => TOAST_TIMES.iter().position(|s| *s == p.toast_s).unwrap_or(0),
            Self::OpusBitrate => OPUS.iter().position(|b| *b == p.opus_bitrate).unwrap_or(1),
            Self::LikeDownload if !p.like_downloads => 2,
            Self::LikeDownload => usize::from(p.like_download_scope == v::WishlistScope::Album),
            Self::UpgradeEvery => v::UpgradeEvery::ALL.iter().position(|e| *e == p.upgrade_every).unwrap_or(1),
            Self::UpgradeBar => usize::from(p.upgrade_below == v::UpgradeBar::Mp3_320),
            Self::UpgradeAuto => usize::from(p.upgrade_auto),
            Self::PlaylistSync => sync_index(p.playlist_sync_default),
            Self::AlbumLike => usize::from(p.album_like_tracks),
            Self::SingleLike => usize::from(p.single_like_saves),
            Self::LikedAlbumsPlace => usize::from(p.liked_albums_with_playlists),
            Self::RadioSource if p.mixes_downloaded_only => 0,
            Self::RadioSource if p.radio_new_share == v::RadioNewShare::None => 1,
            Self::RadioSource => 2,
            Self::RadioNewShare => share_index(p.radio_new_share),
            Self::NewShareBy(kind) => share_index(p.new_share_of(kind)),
            Self::ProfileNames => usize::from(p.profile_names == v::ProfileNames::Artists),
            Self::BackgroundCards => usize::from(p.background_cards.min(3)),
            Self::RadioNoRepeat => NO_REPEAT.iter().position(|h| *h >= p.radio_no_repeat_hours).unwrap_or(NO_REPEAT.len() - 1),
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
            Self::LyricsGpu => p.lyrics_gpu = i == 0,
            Self::LyricsStreamKeep => p.lyrics_stream_keep = i.min(1) as u8,
            Self::LyricsUi => p.lyrics_ui = i.min(2) as u8,
            Self::RingDir => p.ring_ccw = i == 1,
            Self::RingStyle => p.ring_style = [v::RingStyle::Circle, v::RingStyle::Contour, v::RingStyle::Bars][i.min(2)],
            Self::Ambient => p.ambient = [v::Ambient::Off, v::Ambient::Glow, v::Ambient::Reactive][i.min(2)],
            Self::BgShapes => {
                p.bg_shapes = [v::BgShapes::Off, v::BgShapes::Bounce, v::BgShapes::Float, v::BgShapes::Morph][i.min(3)]
            }
            // Picking a colour shows the shapes (as the other fields do).
            Self::BgColors => {
                p.bg_shapes_colors = i.min(3) as u8;
                if p.bg_shapes == v::BgShapes::Off {
                    p.bg_shapes = v::BgShapes::Float;
                }
            }
            Self::BgHue => {
                if p.bg_shapes == v::BgShapes::Off {
                    p.bg_shapes = v::BgShapes::Float;
                }
                p.bg_shapes_colors = 3;
                p.bg_shapes_hue = i.min(7) as u8;
            }
            Self::LyricsStyle => p.lyrics_style = i.min(2) as u8,
            Self::LyricsUnit => p.lyrics_unit = i.min(2) as u8,
            Self::WaveFallback => p.wave_syllables_only = i == 1,
            Self::LyricsGap => p.lyrics_gap_style = i.min(3) as u8,
            Self::NpMotion => p.np_motion = i.min(2) as u8,
            Self::DblClickShape => p.dbl_click_shape = i.min(2) as u8,
            Self::ToastTime => p.toast_s = TOAST_TIMES[i.min(TOAST_TIMES.len() - 1)],
            Self::OpusBitrate => p.opus_bitrate = OPUS[i.min(OPUS.len() - 1)],
            Self::LikeDownload => {
                p.like_downloads = i < 2;
                if i < 2 {
                    p.like_download_scope = if i == 1 { v::WishlistScope::Album } else { v::WishlistScope::Track };
                }
            }
            Self::UpgradeEvery => p.upgrade_every = v::UpgradeEvery::ALL[i.min(3)],
            Self::UpgradeBar => p.upgrade_below = if i == 1 { v::UpgradeBar::Mp3_320 } else { v::UpgradeBar::Lossless },
            Self::UpgradeAuto => p.upgrade_auto = i == 1,
            Self::PlaylistSync => p.playlist_sync_default = v::PlaylistSyncMode::ALL[i.min(2)],
            Self::AlbumLike => p.album_like_tracks = i == 1,
            Self::SingleLike => p.single_like_saves = i == 1,
            Self::LikedAlbumsPlace => p.liked_albums_with_playlists = i == 1,
            Self::RadioSource => {
                p.mixes_downloaded_only = i == 0;
                if i == 1 {
                    p.radio_new_share = v::RadioNewShare::None;
                } else if i == 2 && p.radio_new_share == v::RadioNewShare::None {
                    p.radio_new_share = v::RadioNewShare::Some;
                }
            }
            Self::RadioNewShare => v::PrefPatch::RadioNewShare(SHARES[i.min(2)]).apply(p),
            Self::NewShareBy(kind) => {
                p.radio_new_share_by.insert(kind, SHARES[i.min(2)]);
            }
            Self::ProfileNames => p.profile_names = if i == 1 { v::ProfileNames::Artists } else { v::ProfileNames::Kind },
            Self::BackgroundCards => p.background_cards = i.min(3) as u8,
            Self::RadioNoRepeat => p.radio_no_repeat_hours = NO_REPEAT[i.min(NO_REPEAT.len() - 1)],
            _ => {}
        }
    }

    fn preview(self) -> Option<Preview> {
        use SettingId::*;
        Some(match self {
            Theme | Contrast | Scheme => Preview::Palette,
            Ambient | AmbientStrength => Preview::Ambient,
            ProgressWave | ReducedMotion => Preview::Wave,
            CoverDir | CoverSpin => Preview::Turn(false),
            RingDir | RingSpin => Preview::Turn(true),
            CoverShape | ShapeRange | ShapePick | CoverPulse => Preview::Cover,
            Ring | RingStyle | RingBeat => Preview::Ring,
            LyricsCascade => Preview::Cascade,
            BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue => Preview::Background,
            LyricsStyle | WaveFallback | WaveJump | JumpSpeed | LightSpeed | LyricsSmooth | LyricsDim | LyricsScale | LyricsGlow | LyricsUnit => Preview::Lyrics,
            LyricsSoftEdges | LyricsSoftStrength | LyricsSoftDepth | LyricsSoftFade => Preview::Edges,
            LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize => Preview::Gap,
            LyricsCountdown => Preview::Countdown,
            NpMotion => Preview::Motion,
            NpVolume | NpVolumeHold => Preview::Volume,
            _ => return None,
        })
    }
}

/// The index of `m` in [`v::PlaylistSyncMode::ALL`] (the order of the
/// "Синхронизация" choices).
pub fn sync_index(m: v::PlaylistSyncMode) -> usize {
    v::PlaylistSyncMode::ALL.iter().position(|x| *x == m).unwrap_or(1)
}

/// How long the glow on a linked row lasts.
pub const FOCUS_S: f64 = 1.5;
/// How long the tint on a linked group lasts.
pub const GROUP_S: f64 = 2.4;
/// The opacity of the primary layer tinting a linked group at its fullest.
pub const GROUP_TINT: f32 = 0.12;
/// Frames the scroll to a linked row is repeated, while the page settles.
const SCROLL_FRAMES: u8 = 3;

/// The row a link opened, kept for its glow.
#[derive(Clone, Copy, Debug)]
struct Focus {
    id: SettingId,
    start: f64,
    scrolls: u8,
    rect: Option<Rect>,
    /// The whole group of the row is shown (tinted), not the row.
    group: bool,
}

/// The group being drawn while a link asks for a whole group: its tint is
/// reserved behind its title and painted once its end is known.
#[derive(Clone, Copy, Debug)]
struct Span {
    pass: u64,
    layer: egui::LayerId,
    bg: egui::layers::ShapeIdx,
    clip: Rect,
    x: egui::Rangef,
    top: f32,
    /// It holds the linked row.
    linked: bool,
}

/// What a row reports about its insides while it is drawn.
#[derive(Clone, Debug, Default)]
struct Parts {
    button: Option<Rect>,
    control: Option<Rect>,
    choices: Vec<Rect>,
    menu: Vec<Rect>,
}

/// One row drawn this pass, for the cards.
#[derive(Clone, Debug)]
struct Row {
    id: SettingId,
    label: String,
    rect: Rect,
    button: Rect,
    control: Option<Rect>,
    /// Its segmented buttons, and the items of its open menu.
    choices: Vec<Rect>,
    menu: Vec<Rect>,
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
    /// What it shows (an option while the pointer is on its button) and
    /// since when, for the cross-fade.
    body: Body,
    changed: f64,
    /// Where the card was last drawn.
    card: Option<Rect>,
}

/// What a card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    /// The row's explanation and its current preview.
    Row,
    /// One option of a choice, previewed.
    Option(usize),
    /// The explanation and the options as lines (nothing to preview).
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CardKey {
    id: SettingId,
    body: Body,
    /// About an item of the row's open menu (the card goes beside it).
    menu: bool,
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
    /// The pointer last frame, for its speed.
    last: Option<(Pos2, f64)>,
}

fn focus_key() -> Id {
    Id::new("settings-focus")
}

fn span_key() -> Id {
    Id::new("settings-focus-span")
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

/// Starts the scroll and glow for `id` (the page then switches its tab);
/// with `group`, for the whole group that holds it.
pub fn begin_focus(ctx: &egui::Context, id: SettingId, group: bool) {
    let start = anim::now(ctx);
    ctx.data_mut(|d| d.insert_temp(focus_key(), Focus { id, start, scrolls: SCROLL_FRAMES, rect: None, group }));
}

/// The tint (0..1) of a linked group `t` seconds after the link: it comes
/// up on the M3 slow-effects spring once the page has landed, holds while
/// the eye finds it, and eases back out by [`GROUP_S`]. Reduced motion
/// shows it at once and only fades it out.
pub fn group_tint_at(t: f32, reduced: bool) -> f32 {
    const DELAY: f32 = 0.15;
    const HOLD: f32 = 1.2;
    let total = GROUP_S as f32;
    if !(0.0..total).contains(&t) {
        return 0.0;
    }
    let rise = if reduced {
        1.0
    } else {
        let t = (t - DELAY).max(0.0);
        if t <= 0.0 { 0.0 } else { 1.0 - nikgob_motion::tokens::SLOW_EFFECTS.solve(1.0, 0.0, t).0 }
    };
    let out = ((t - HOLD) / (total - HOLD)).clamp(0.0, 1.0);
    rise.clamp(0.0, 1.0) * (1.0 - out * out * (3.0 - 2.0 * out))
}

/// Called as each settings group starts (before its title): ends the
/// group before it and, while a link asks for a whole group, reserves the
/// new group's tint behind its title.
pub fn group_start(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    end_group(ui);
    if ctx.data(|d| d.get_temp::<Focus>(focus_key())).is_some_and(|f| f.group) {
        let span = Span {
            pass: ctx.cumulative_pass_nr(),
            layer: ui.layer_id(),
            bg: ui.painter().add(Shape::Noop),
            clip: ui.clip_rect(),
            x: ui.max_rect().x_range(),
            // The title comes after a 24 px space.
            top: ui.cursor().top() + 12.0,
            linked: false,
        };
        ctx.data_mut(|d| d.insert_temp(span_key(), span));
    }
}

/// Ends the group being drawn here: the linked one gets its tint and is
/// where the page scrolls.
fn end_group(ui: &Ui) {
    let ctx = ui.ctx();
    let Some(span) = ctx.data_mut(|d| {
        let span = d.get_temp::<Span>(span_key());
        d.remove::<Span>(span_key());
        span
    }) else {
        return;
    };
    if span.pass != ctx.cumulative_pass_nr() || !span.linked {
        return;
    }
    let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key())) else { return };
    let rect = Rect::from_x_y_ranges(span.x, span.top..=ui.cursor().top() + 8.0).expand2(vec2(12.0, 0.0));
    let theme = Theme::get(ctx);
    let a = group_tint_at((anim::now(ctx) - f.start) as f32, theme.reduced_motion);
    if a > 0.0 {
        // A primary tonal layer over the page, as a selected surface has:
        // not secondary_container, which is the sliders' own track colour
        // and would hide it.
        let painter = Painter::new(ctx.clone(), span.layer, Rect::from_x_y_ranges(rect.x_range(), span.clip.y_range()));
        painter.set(span.bg, Shape::rect_filled(rect, shape::LARGE, theme.scheme.primary.gamma_multiply(GROUP_TINT * a)));
    }
    if (anim::now(ctx) - f.start) < GROUP_S {
        ctx.request_repaint();
    }
    f.rect = Some(rect);
    ctx.data_mut(|d| d.insert_temp(focus_key(), f));
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
    if let Some(f) = ctx.data(|d| d.get_temp::<Focus>(focus_key()))
        && f.id == id
        && f.group
    {
        ctx.data_mut(|d| {
            if let Some(mut span) = d.get_temp::<Span>(span_key()) {
                span.linked = true;
                d.insert_temp(span_key(), span);
            }
        });
    } else if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key()))
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
        control: parts.control,
        choices: parts.choices,
        menu: parts.menu,
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

/// The ⓘ button of the item being drawn, just after `beside` (a control
/// that fills its row, such as a connected group) and on its centre line.
pub fn help_button_beside(ui: &mut Ui, id: SettingId, beside: Rect) {
    let rect = Rect::from_min_size(pos2(beside.right() + 8.0, beside.center().y - BTN * 0.5), Vec2::splat(BTN));
    help_button_at(ui, id, rect);
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).button = Some(rect));
}

/// The rects of the item's open menu, in [`SettingId::choices`] order.
pub fn report_menu(ctx: &egui::Context, rects: Vec<Rect>) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).menu = rects);
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
        set_pinned(ctx, (!was).then(|| Pinned { id, opened: ctx.cumulative_pass_nr(), at, body: Body::Row, changed: at, card: None }));
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
    end_group(ui);
    if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key())) {
        if let Some(rect) = f.rect.take()
            && f.scrolls > 0
        {
            // After the controls, so their nested scroll areas cannot take
            // it. A group taller than the view shows from its title.
            if f.group && rect.height() > ui.clip_rect().height() * 0.8 {
                ui.scroll_to_rect(Rect::from_min_size(rect.min - vec2(0.0, 16.0), vec2(rect.width(), 1.0)), Some(Align::Min));
            } else {
                ui.scroll_to_rect(rect.expand(24.0), Some(Align::Center));
            }
            f.scrolls -= 1;
            ctx.request_repaint();
        }
        let lasts = if f.group { GROUP_S } else { FOCUS_S };
        if f.scrolls == 0 && anim::now(&ctx) - f.start > lasts {
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
        // The row's current option, or the option whose button the
        // pointer is on.
        let now = anim::now(&ctx);
        let pointer = ctx.pointer_hover_pos();
        let n = row.id.choices().len();
        let body = match pointer.and_then(|p| row.choices.iter().position(|r| r.contains(p))).filter(|i| *i < n) {
            Some(i) => Body::Option(i),
            None => base_body(row.id),
        };
        if body != pin.body {
            pin.body = body;
            pin.changed = now;
        }
        let content = ((now - pin.changed) / 0.14).clamp(0.0, 1.0) as f32;
        if content < 1.0 {
            ctx.request_repaint();
        }
        let layout = Layout::new(&ctx, row, body, true);
        // Stays where it opened while its content swaps.
        let x = row.control.map_or(row.button.left(), |r| r.left());
        let target = Rect::from_min_size(place(row.rect, x, layout.size, ctx.content_rect()), layout.size);
        // A short fade and rise from the row (effects timing, no bounce).
        let t = ((anim::now(&ctx) - pin.at) / 0.18).clamp(0.0, 1.0) as f32;
        let k = 1.0 - (1.0 - t).powi(3);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let rise = if target.top() >= row.rect.bottom() { -1.0 } else { 1.0 } * 8.0 * (1.0 - k);
        let rect = target.translate(vec2(0.0, -rise));
        let out = show_card(&ctx, Id::new("settings-help-pinned-card"), rect, k, content, &layout, row, prefs, true);
        if let Some(i) = out.pick {
            row.id.choose(prefs, i);
        }
        pin.card = Some(rect);
        set_pinned(&ctx, (!out.close).then_some(pin));
        return;
    }
    hover(&ctx, &rows, prefs);
}

/// First card: the pointer rests this long on an option or an ⓘ.
const ENTER_DELAY: f64 = 0.3;
/// With a card up, resting this long on another one switches to it.
const SWITCH_DELAY: f64 = 0.12;
/// Faster than this (px/s) the pointer is passing by, not resting.
const PASSING: f32 = 300.0;
/// The card waits this long after the pointer leaves.
const LEAVE_GRACE: f64 = 0.15;

/// What the pointer at `p` asks for: an option button or menu item of a
/// row (that option), or its ⓘ (every option, else the row). Labels,
/// sliders and switches ask for nothing.
fn key_at(row: &Row, p: Pos2) -> Option<CardKey> {
    let n = row.id.choices().len();
    if let Some(i) = row.menu.iter().position(|r| r.contains(p)).filter(|i| *i < n) {
        return Some(CardKey { id: row.id, body: Body::Option(i), menu: true });
    }
    if !row.hovered {
        return None;
    }
    if let Some(i) = row.choices.iter().position(|r| r.contains(p)).filter(|i| *i < n) {
        return Some(CardKey { id: row.id, body: Body::Option(i), menu: false });
    }
    row.button.contains(p).then(|| CardKey { id: row.id, body: base_body(row.id), menu: false })
}

/// What the ⓘ card of `id` shows: the explanation and the current
/// option's preview, or the options as lines when nothing is previewed.
fn base_body(id: SettingId) -> Body {
    if !id.choices().is_empty() && id.preview().is_none() { Body::All } else { Body::Row }
}

/// The hover card: only for an option button, a menu item or an ⓘ the
/// pointer rests on (a pass over them opens nothing). With a card up, a
/// short rest on a neighbour switches it, gliding and cross-fading; it
/// stays while the pointer is on it and a moment after the pointer leaves.
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
        last: None,
    });
    let pointer = ctx.pointer_hover_pos();
    let speed = match (pointer, h.last) {
        (Some(p), Some((q, t))) if now > t => p.distance(q) / (now - t) as f32,
        _ => 0.0,
    };
    h.last = pointer.map(|p| (p, now));
    // On the card itself: keep what it shows.
    let on_card = h.open && pointer.is_some_and(|p| h.drawn.is_some_and(|r| r.contains(p)));
    let want = if on_card { h.key } else { pointer.and_then(|p| rows.iter().find_map(|r| key_at(r, p))) };
    if want.is_some() {
        h.left = None;
    }
    if h.candidate != want || speed > PASSING {
        h.candidate = want;
        h.since = now;
    }
    let rested = now - h.since;
    let showing = h.open || h.alpha.value(now) > 0.05;
    match want {
        Some(k) if showing => {
            if h.key != Some(k) {
                if rested >= SWITCH_DELAY {
                    if h.key.is_some() {
                        h.content.snap(0.0);
                        h.content.set_target(1.0, now);
                    }
                    if h.key.is_some_and(|o| o.menu != k.menu) {
                        h.drawn = None;
                    }
                    h.key = Some(k);
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(SWITCH_DELAY - rested));
                }
            }
            if !h.open && h.key == Some(k) {
                h.open = true;
                h.alpha.set_target(1.0, now);
            }
        }
        Some(k) => {
            if rested >= ENTER_DELAY {
                h.key = Some(k);
                h.open = true;
                h.drawn = None;
                h.content.snap(1.0);
                h.alpha.snap(0.0);
                h.alpha.set_target(1.0, now);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((ENTER_DELAY - rested).max(0.01)));
            }
        }
        None => {
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
            let screen = ctx.content_rect();
            let item = match key.body {
                Body::Option(i) if key.menu => row.menu.get(i).copied(),
                _ => None,
            };
            let target = match item {
                Some(item) => Rect::from_min_size(beside(&row.menu, item, layout.size, screen), layout.size),
                None => Rect::from_min_size(place(row.rect, layout.anchor_x, layout.size, screen), layout.size),
            };
            let goal = [target.left(), target.top(), target.width(), target.height()];
            if h.drawn.is_none() {
                // Appearing: rise a little from the row's side.
                let rise = if item.is_some() { 0.0 } else if target.top() >= row.rect.bottom() { -8.0 } else { 8.0 };
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

/// Where a card about a menu item goes: beside the open menu (right, else
/// left), level with the item, inside `screen`.
fn beside(menu: &[Rect], item: Rect, size: Vec2, screen: Rect) -> Pos2 {
    let screen = screen.shrink2(vec2(8.0, 4.0));
    let all = menu.iter().fold(item, |a, r| a.union(*r)).expand(8.0);
    let x = if all.right() + GAP + size.x <= screen.right() { all.right() + GAP } else { (all.left() - GAP - size.x).max(screen.left()) };
    let y = item.top().clamp(screen.top(), (screen.bottom() - size.y).max(screen.top()));
    pos2(x, y)
}

/// Text width of a one-thing card.
const CARD_W: f32 = 280.0;
const PREVIEW_H: f32 = 120.0;
const PAD: f32 = 12.0;
/// Between a row and its card.
const GAP: f32 = 6.0;

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

/// A card laid out: its size, where it lines up, and its texts.
struct Layout {
    size: Vec2,
    anchor_x: f32,
    body: Body,
    kind: Option<Preview>,
    title: Arc<Galley>,
    help: Arc<Galley>,
    /// [`Body::All`]: each option's name and what it does.
    texts: Vec<(Arc<Galley>, Arc<Galley>)>,
    /// A close button in the header (the pinned card).
    close: bool,
}

/// The first words of a [`SettingId::beta`] row's card.
const BETA_CARD: &str = "Функция в бета-тестировании. ";

/// Room for the close button beside the title.
const CLOSE_W: f32 = 32.0;

impl Layout {
    fn new(ctx: &egui::Context, row: &Row, body: Body, pinned: bool) -> Self {
        let s = Theme::get(ctx).scheme;
        let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("settings-help-measure")));
        let choices = row.id.choices();
        let kind = if body == Body::All { None } else { row.id.preview() };
        let (title, help) = match body {
            Body::Option(i) => (format!("{} · {}", row.label, choices[i].label), choices[i].help.to_owned()),
            _ if row.id.beta() => (row.label.clone(), BETA_CARD.to_owned() + row.id.help()),
            _ => (row.label.clone(), row.id.help().to_owned()),
        };
        let close_w = if pinned { CLOSE_W } else { 0.0 };
        let title = para(&painter, TypeRole::TitleSmall, &title, s.on_surface, CARD_W - close_w);
        let help = para(&painter, TypeRole::BodyMedium, &help, s.on_surface_variant, CARD_W);
        let texts: Vec<(Arc<Galley>, Arc<Galley>)> = if body == Body::All {
            choices
                .iter()
                .map(|c| {
                    (
                        para(&painter, TypeRole::LabelLarge, c.label, s.on_surface, CARD_W - 28.0),
                        para(&painter, TypeRole::BodySmall, c.help, s.on_surface_variant, CARD_W - 28.0),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let header_h = title.size().y.max(if pinned { 24.0 } else { 0.0 });
        let mut h = PAD + header_h + 2.0 + help.size().y;
        if kind.is_some() {
            h += 10.0 + PREVIEW_H;
        }
        for (lg, hg) in &texts {
            h += 8.0 + lg.size().y + hg.size().y;
        }
        h += PAD;
        let anchor_x = match body {
            Body::Option(i) => row.choices.get(i).map(|r| r.left()),
            _ => None,
        }
        .or(row.control.map(|r| r.left()))
        .unwrap_or(row.button.left());
        Self { size: vec2(CARD_W + 2.0 * PAD, h), anchor_x, body, kind, title, help, texts, close: pinned }
    }
}

/// What a card's clicks did.
struct CardOut {
    pick: Option<usize>,
    close: bool,
}

/// Paints `layout` into `rect` (which may still be gliding to its size) at
/// `alpha`, the content at `content`; `interactive` cards take clicks.
/// Title (with a close button when pinned), explanation, then the preview
/// of the option it is about (the current one for the row), or the
/// options as lines when there is nothing to preview.
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
            let x0 = rect.left() + PAD;
            let mut y = rect.top() + PAD;
            let header_h = layout.title.size().y.max(if layout.close { 24.0 } else { 0.0 });
            if layout.close {
                let close_r = Rect::from_center_size(pos2(rect.right() - PAD - 10.0, y + header_h * 0.5), Vec2::splat(28.0));
                let resp = ui.interact(close_r, area.with("close"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                if resp.hovered() {
                    ui.painter().circle_filled(close_r.center(), 14.0, s.on_surface.gamma_multiply(0.08));
                }
                Icon::Close.widget(false, s.on_surface_variant, 18.0).paint_at(ui, Rect::from_center_size(close_r.center(), Vec2::splat(18.0)));
                out.close = resp.clicked();
            }
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.multiply_opacity(content);
            let ui = &mut ui;
            let th = layout.title.size().y;
            ui.painter().galley(pos2(x0, y + (header_h - th) * 0.5), layout.title.clone(), s.on_surface);
            y += header_h + 2.0;
            ui.painter().galley(pos2(x0, y), layout.help.clone(), s.on_surface_variant);
            y += layout.help.size().y;
            if let Some(kind) = layout.kind {
                y += 10.0;
                let r = Rect::from_min_size(pos2(x0, y), vec2(CARD_W, PREVIEW_H));
                match layout.body {
                    Body::Option(i) => {
                        sample(ui, r, kind, &with_choice(row.id, prefs, i), Some(Id::new(("card", row.id, i))));
                    }
                    // A choice shows its current option exactly; a slider or
                    // switch may show a stand-in while its effect is off.
                    _ if !row.id.choices().is_empty() => {
                        let i = row.id.chosen(prefs);
                        sample(ui, r, kind, &with_choice(row.id, prefs, i), Some(Id::new(("card", row.id, i))));
                    }
                    _ => sample(ui, r, kind, prefs, None),
                }
            }
            if !layout.texts.is_empty() {
                // The options as lines: a radio mark, the name, what it does.
                let chosen = row.id.chosen(prefs);
                for (i, (lg, hg)) in layout.texts.iter().enumerate() {
                    y += 8.0;
                    let rh = lg.size().y + hg.size().y;
                    let r = Rect::from_min_size(pos2(x0 - 4.0, y - 4.0), vec2(CARD_W + 8.0, rh + 8.0));
                    let sel = i == chosen;
                    if interactive {
                        let resp = ui.interact(r, area.with(("opt", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                        if resp.hovered() && !sel {
                            ui.painter().rect_filled(r, shape::SMALL, s.on_surface.gamma_multiply(0.06));
                        }
                        if resp.clicked() && !sel {
                            out.pick = Some(i);
                        }
                    }
                    let dot = pos2(x0 + 8.0, y + lg.size().y * 0.5);
                    ui.painter().circle_stroke(dot, 6.0, Stroke::new(2.0, if sel { s.primary } else { s.on_surface_variant }));
                    if sel {
                        ui.painter().circle_filled(dot, 3.0, s.primary);
                    }
                    let lh = lg.size().y;
                    ui.painter().galley(pos2(x0 + 24.0, y), lg.clone(), if sel { s.primary } else { s.on_surface });
                    ui.painter().galley(pos2(x0 + 24.0, y + lh), hg.clone(), s.on_surface_variant);
                    y += rh;
                }
            }
        });
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
            let wavy = prefs.progress_wave && !theme.reduced_motion && !prefs.reduced_motion;
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
        // The ring's look: no countdown over it.
        Preview::Gap => lyrics::gap_preview(ui, rect, &Prefs { lyrics_countdown: false, ..prefs.clone() }),
        // What turning it on gives, whatever the switch says now.
        Preview::Countdown => lyrics::countdown_preview(ui, rect, &Prefs { lyrics_countdown: true, ..prefs.clone() }),
        Preview::Motion => motion_preview(ui, rect, prefs),
        Preview::Volume => volume_preview(ui, rect, prefs),
        Preview::Turn(ring) => turn_preview(ui, rect, prefs, ring),
        Preview::Cascade => cascade_preview(ui, rect, prefs),
        Preview::Edges => lyrics::edges_preview(ui, rect, prefs),
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
    // A second of changing the volume, then the pill's hold, then a rest.
    let hold = prefs.np_volume_pop_hold.clamp(0.3, 5.0);
    let cycle = 1.0 + hold + 0.7;
    let t = (anim::now(ui.ctx()) as f32).rem_euclid(cycle);
    let changing = t.min(1.0);
    let level = 0.35 + 0.3 * (changing * 2.1).sin().abs();
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
        // Rises while the volume changes, stays for the hold, then hides.
        let k = ((t / 0.2).min((1.0 + hold + 0.25 - t) / 0.25)).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        let w = (rect.width() * 0.6).min(160.0);
        let h = 20.0;
        let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - pad - h * 0.5 + (1.0 - k) * 10.0), vec2(w, h));
        // Where it appears, faintly, while it is hidden.
        painter.rect_stroke(pill, h * 0.5, Stroke::new(1.0, s.outline_variant.gamma_multiply(0.6 * (1.0 - k))), egui::StrokeKind::Inside);
        painter.rect_filled(pill, h * 0.5, s.surface_container_highest.gamma_multiply(k));
        bar(&painter, pos2(pill.left() + 12.0, pill.center().y), w - 24.0, k);
    } else {
        // The wheel: a small mouse beside the button.
        let m = Rect::from_center_size(button + vec2(r + 16.0, 0.0), vec2(12.0, 18.0));
        painter.rect_stroke(m, 6.0, Stroke::new(1.5, s.on_surface_variant), egui::StrokeKind::Inside);
        let wy = m.top() + 4.0 + 3.0 * (anim::now(ui.ctx()) as f32 * 4.0).sin().abs();
        painter.line_segment([pos2(m.center().x, wy), pos2(m.center().x, wy + 3.0)], Stroke::new(2.0, s.primary));
    }
}

/// Which way the cover's outline (or the ring) turns: a clock face, the
/// shape turning in it with a bright marker, and an arrow running ahead of
/// the marker the same way. Clockwise goes 12 → 3 → 6.
fn turn_preview(ui: &Ui, rect: Rect, prefs: &Prefs, ring: bool) {
    use std::f32::consts::TAU;
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let ccw = if ring { prefs.ring_ccw } else { prefs.cover_ccw };
    let sign = if ccw { -1.0 } else { 1.0 };
    // Not turning at all: no marker run, no arrow.
    let turning = if ring { prefs.spin_ring } else { prefs.cover_spin };
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5 - 18.0;
    let a = if turning { sign * (anim::now(ui.ctx()) as f32 * TAU / 6.0) } else { 0.0 };
    let dir = |th: f32| vec2(th.sin(), -th.cos());
    // Clock hints: 12, 3, 6, 9.
    let font = TypeRole::LabelSmall.style().font_id();
    for (i, n) in ["12", "3", "6", "9"].iter().enumerate() {
        let th = i as f32 * TAU / 4.0;
        painter.text(c + dir(th) * (r + 11.0), egui::Align2::CENTER_CENTER, *n, font.clone(), s.on_surface_variant.gamma_multiply(0.7));
    }
    if ring {
        for i in 0..24 {
            let th = a + i as f32 * TAU / 24.0;
            painter.circle_filled(c + dir(th) * (r - 4.0), 2.0, s.primary.gamma_multiply(0.45));
        }
        painter.circle_filled(c, r * 0.55, s.surface_container_highest);
    } else {
        let shape = match prefs.cover_shape {
            v::CoverShape::Original | v::CoverShape::Random => v::CoverShape::Cookie12,
            other => other,
        };
        if let Some(m) = now_playing::material(shape) {
            let box_ = Rect::from_center_size(c, Vec2::splat((r - 6.0) * 2.0));
            nikgob_shapes::paint::fill_shape(&painter, now_playing::shape_cubics(m), box_, a.rem_euclid(TAU), s.secondary_container);
        }
    }
    // The marker on the turning part, and an arrow ahead of it.
    let mr = if ring { r - 4.0 } else { r - 12.0 };
    painter.circle_filled(c + dir(a) * mr, 4.5, s.primary);
    if !turning {
        return;
    }
    let ar = r + 1.0;
    let span = 0.9;
    let pts: Vec<Pos2> = (0..=16).map(|k| c + dir(a + sign * (0.25 + span * k as f32 / 16.0)) * ar).collect();
    let stroke = Stroke::new(2.0, s.tertiary);
    painter.add(Shape::line(pts.clone(), stroke));
    // Arrowhead at the end, along the way of the turn.
    let tip = *pts.last().unwrap_or(&c);
    let th = a + sign * (0.25 + span);
    let along = vec2(th.cos(), th.sin()) * sign;
    let out = dir(th);
    painter.add(Shape::convex_polygon(
        vec![tip + along * 6.0, tip - along * 2.0 + out * 4.5, tip - along * 2.0 - out * 4.5],
        s.tertiary,
        Stroke::NONE,
    ));
}

/// Lines of text scrolling up one line at a time; with the cascade the
/// lines further from the sung one follow later, like a wave.
fn cascade_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const STEP: f32 = 1.4;
    let t = anim::now(ui.ctx()) as f32;
    let (n, f) = ((t / STEP).floor(), (t / STEP).fract() * STEP);
    let line_h = (rect.height() / 5.0).clamp(14.0, 26.0);
    let cur_y = rect.center().y;
    let widths = [0.8, 0.55, 0.7, 0.45, 0.65, 0.75, 0.5, 0.6];
    for k in -3i32..=4 {
        // Line `idx` sits `k` lines below the current one after this step.
        let idx = (n as i32 + k).rem_euclid(widths.len() as i32) as usize;
        let delay = if prefs.lyrics_cascade { (k.unsigned_abs() as f32) * 0.12 } else { 0.0 };
        let x = ((f - delay) / 0.45).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - x).powi(3);
        let y = cur_y + (k as f32 + 1.0 - ease) * line_h;
        let current = k == 0;
        let w = (rect.width() - 24.0) * widths[idx];
        let bar = Rect::from_min_size(pos2(rect.left() + 12.0, y - line_h * 0.22), vec2(w, line_h * 0.44));
        let color = if current { s.on_surface } else { s.on_surface.gamma_multiply(0.35) };
        painter.rect_filled(bar, line_h * 0.22, color);
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
    fn group_tint_comes_up_holds_and_eases_out() {
        assert_eq!(group_tint_at(-0.1, false), 0.0);
        assert_eq!(group_tint_at(0.1, false), 0.0, "waits for the page to land");
        assert!(group_tint_at(0.3, false) > 0.5);
        assert!(group_tint_at(1.0, false) > 0.99);
        assert!(group_tint_at(2.2, false) < 0.2);
        assert_eq!(group_tint_at(GROUP_S as f32, false), 0.0);
        let mut was = 1.0;
        for ms in 1200..2400 {
            let v = group_tint_at(ms as f32 / 1000.0, false);
            assert!(v <= was);
            was = v;
        }
        // Reduced motion: there at once, then only the fade.
        assert_eq!(group_tint_at(0.0, true), 1.0);
        assert_eq!(group_tint_at(1.0, true), 1.0);
        assert!(group_tint_at(2.2, true) < 0.2);
    }

    #[test]
    fn linked_rows_know_their_tab_and_every_row_has_help() {
        assert_eq!(SettingId::LyricsStyle.tab(), TAB_APPEARANCE);
        assert_eq!(SettingId::BgHue.tab(), TAB_APPEARANCE);
        // Torrents moved to «Источники»: old links land there.
        assert_eq!(SettingId::Prowlarr.tab(), TAB_SOURCES);
        assert_eq!(SettingId::NoSeed.tab(), TAB_SOURCES);
        assert_eq!(SettingId::Spotify.tab(), TAB_SOURCES);
        assert_eq!(SettingId::MusicBrainz.tab(), TAB_GENERAL);
        assert_eq!(SettingId::RadioNewShare.tab(), TAB_RECS);
        assert_eq!(SettingId::SoundAnalysis.tab(), TAB_RECS);
        assert_eq!(SettingId::StreamAnalysis.tab(), TAB_SOURCES);
        assert_eq!(SettingId::MixesDownloadedOnly.tab(), TAB_RECS);
        assert_eq!(SettingId::Autoplay.tab(), TAB_RECS);
        assert_eq!(SettingId::DislikeInRecs.tab(), TAB_RECS);
        assert_eq!(SettingId::NewShareBy(v::RadioKind::Mix).tab(), TAB_RECS);
        assert_eq!(SettingId::BackgroundCountsLess.tab(), TAB_RECS);
        assert_eq!(SettingId::ProfilesSplit.tab(), TAB_RECS);
        assert_eq!(SettingId::ProfileNames.tab(), TAB_RECS);
        assert_eq!(SettingId::BackgroundCards.tab(), TAB_RECS);
        assert_eq!(SettingId::MixesEverything.tab(), TAB_RECS);
        for id in [SettingId::GraphUse, SettingId::GraphServices, SettingId::GraphOpenData, SettingId::GraphPersonal, SettingId::LibraryAsTaste, SettingId::LastfmKey] {
            assert_eq!(id.tab(), TAB_RECS);
            assert!(!id.help().is_empty());
        }
        assert!(!SettingId::Autoplay.help().is_empty() && !SettingId::RadioSource.help().is_empty());
        assert_eq!(SettingId::RadioNoRepeat.tab(), TAB_RECS);
        assert!(!SettingId::RadioNoRepeat.help().is_empty());
        for id in [SettingId::RadioNewShare, SettingId::MixesDownloadedOnly, SettingId::SoundAnalysis, SettingId::StreamAnalysis] {
            assert!(!id.help().is_empty());
        }
        for id in [SettingId::SpotifyPersonal, SettingId::YouTubeCookies, SettingId::ImportFiles, SettingId::ImportSavedSongs, SettingId::AutoDownloads] {
            assert!(!id.help().is_empty());
        }
        for id in [SettingId::LyricsUi, SettingId::LyricsLinesAuto, SettingId::LyricsWordsPlaying, SettingId::LyricsWordsAll] {
            assert_eq!(id.tab(), TAB_GENERAL);
            assert!(!id.help().is_empty());
        }
        for id in [SettingId::Design, SettingId::FpsLimit, SettingId::Nodes, SettingId::RingBeat] {
            assert!(!id.help().is_empty());
        }
    }

    #[test]
    fn every_choice_round_trips_through_prefs() {
        use SettingId as S;
        for id in [
            S::Theme, S::Contrast, S::Scheme, S::CoverShape, S::CoverDir, S::RingDir, S::RingStyle, S::Ambient,
            S::BgShapes, S::BgColors, S::LyricsStyle, S::LyricsUnit, S::WaveFallback, S::LyricsGap, S::NpMotion, S::DblClickShape,
            S::OpusBitrate, S::NpVolume, S::LikeDownload, S::AlbumLike, S::SingleLike, S::LikedAlbumsPlace, S::PlaylistSync, S::RadioNewShare, S::RadioSource,
            S::NewShareBy(v::RadioKind::Track), S::NewShareBy(v::RadioKind::Autoplay), S::ProfileNames, S::BackgroundCards,
            S::RadioNoRepeat, S::LyricsGpu, S::LyricsUi, S::ToastTime, S::LyricsStreamKeep,
            S::UpgradeEvery, S::UpgradeBar, S::UpgradeAuto,
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
