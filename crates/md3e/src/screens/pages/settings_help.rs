//! What every settings row is: an id other screens can link to ("настроить"
//! opens Settings on that row, scrolls to it and makes it glow), its name,
//! tab and level («Просто», «Подробно», «Для разработчика»), a short
//! explanation and, for the looks, a small live preview painted by the real
//! painters.
//!
//! Nothing explains itself on hover: the small ⓘ button next to the label
//! pins a card open with the explanation and, for a choice, every option
//! (the one under the pointer previewed; beside the live stage the card
//! has no sample of its own). The card closes on the button, a click
//! outside or Esc.
//!
//! A new row opts in by getting a [`SettingId`] variant with its
//! [`SettingId::tab`], [`SettingId::level`], [`SettingId::label`] and
//! [`SettingId::help`] (a choice also [`SettingId::choices`]);
//! [`super::settings`] does the rest.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    Align, Area, Color32, CursorIcon, Galley, Id, Key, Modifiers, Order, Painter, Pos2, Rect, Sense, Shape, Stroke,
    Ui, Vec2, pos2, text::LayoutJob, vec2,
};
use nikgob_view::{self as v, Prefs};

use super::home::Section;
use crate::screens::stage::{Part, Scene, Script};
use crate::screens::{bg_shapes, lyrics, now_playing};
use crate::{
    ColorScheme, Contrast, Elevation, Icon, SchemeVariant, Theme, TypeRole,
    components::{anim, paint, paint::Radii, slider::wave_line},
    tokens::shape,
};

/// One settings row (or block of fields).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingId {
    // Вид.
    Language,
    Theme,
    Contrast,
    Scheme,
    ProgressWave,
    NpVolume,
    NpVolumeHold,
    CoverShape,
    ShapeRange,
    ShapePick,
    /// «Менять форму на паузе».
    CoverPauseMorph,
    CoverSpin,
    CoverDir,
    CoverPulse,
    /// «Обложка → Скорость»: seconds a turn of the cover's outline takes.
    CoverSpinSpeed,
    /// «Пульсация → Сила».
    CoverPulseStrength,
    /// «Кольцо»: around a shaped cover while it plays; it turns and jumps
    /// with the music on its own (no rows of its own for those).
    Ring,
    RingStyle,
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
    /// «Подъём спетого»: under karaoke the sung words float up a little.
    LyricsLift,
    LyricsGap,
    LyricsGapWaves,
    /// «Высота волн» and «Размер индикатора» of the pause ring.
    LyricsGapHeight,
    LyricsGapSize,
    LyricsCountdown,
    /// «Шаг подсветки»: by letters or by words, under every animation.
    LyricsUnit,
    /// «Мягкие края» of the lyrics and their strength.
    LyricsSoftEdges,
    LyricsSoftStrength,
    /// «Глубина мягких краёв» and «Затухание».
    LyricsSoftDepth,
    LyricsSoftFade,
    /// «Масштаб интерфейса»: the whole window larger or smaller.
    UiScale,
    RailCompact,
    /// «Дата альбома»: year, month or the whole date.
    AlbumDate,
    /// The parts of the side pane: «Текст песни», «Источник и качество».
    PaneLyrics,
    PaneInfo,
    /// «Меню трека»: where each item of a track's menu stands.
    TrackMenu,
    /// The switch of an optional button of Now Playing and the player bar.
    PlayerButton(v::PlayerButton),
    /// «Разделы открываются с начала»: «Главная» and «Библиотека» of the
    /// rail open the section's first page.
    NavRoot,
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
    /// «Уведомления на экране»: how long a snackbar stays.
    ToastTime,
    SaveLyrics,
    LyricsReturn,
    LyricsCheck,
    /// «Строки простого текста».
    LyricsLinesAuto,
    /// «Вид»: what the control under the lyrics opens.
    LyricsUi,
    /// «Подсветка по словам в играющей песне».
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
    /// «Перемешивание» of a new list of one kind: shuffled, in order, or
    /// as the button is.
    ListShuffle(v::ListKind),
    /// «Перемешивать радио и автовоспроизведение».
    ShuffleRadio,
    OutputDevice,
    Gapless,
    Crossfade,
    Normalization,
    /// «Эквалайзер»: its switch, and the sheet with the curve.
    Equalizer,
    /// «Скорость трека»: the «Скорость» item of a track's menu.
    TrackSpeed,
    /// «Сохранять тембр голоса» of a track played faster or slower.
    KeepTimbre,
    LibraryDir,
    MastersDir,
    Inbox,
    Trash,
    ExportData,
    OpusBitrate,
    /// «Ненастоящий FLAC»: not checked, marked, asked about or made MP3
    /// by itself (three prefs as one choice).
    FakeFlac,
    /// «Вернуть FLAC из Корзины».
    FakeRestore,
    MusicBrainz,
    AcoustId,
    /// «Демо-профиль» in «Для разработчика» (own builds only).
    DemoProfile,
    /// «Показать первый запуск» in «Для разработчика».
    FirstRun,
    /// «Как к тебе обращаться».
    UserName,
    /// «Новое в радио и миксах».
    RadioNewShare,
    /// «Только из скачанного».
    MixesDownloadedOnly,
    /// «Автовоспроизведение».
    Autoplay,
    /// «Продолжать мир после альбома».
    AutoplayWorld,
    /// «Трек из поиска»: «Радио по треку» or «Остальные результаты поиска».
    SearchPlay,
    /// Where radios pick from: «Только из скачанного» and «Новое в радио и
    /// миксах» as one choice. Only the queue's radio header shows it; it has
    /// no row in Settings.
    RadioSource,
    /// «Не повторять сыгранное» in the radio and autoplay.
    RadioNoRepeat,
    /// «Подмешивать случайное» in the radio and autoplay.
    RadioExplore,
    /// «Преимущественно новые песни» in the radio and autoplay.
    RadioUnheard,
    /// «Нейросетевой анализ звука».
    SoundNeural,
    /// «На чём считать анализ звука».
    SoundDevice,
    /// «Карточки в „Миксах для тебя“»: they start «Моя волна», open their
    /// list or play it (one choice: the older «Нажатие на микс открывает
    /// его страницу» is one of its answers now).
    MixCardsWave,
    /// «Моя волна» in the header of Главная.
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
    /// «Кнопка «Не нравится»».
    DislikeButton,
    /// «Пропускать по дизлайку».
    DislikeSkips,
    /// «Пропускать и в своём».
    DislikeSkipEverywhere,
    /// «Учитывать в рекомендациях».
    DislikeInRecs,
    /// «Не нравится и скорость»: the button of the page with the disliked
    /// tracks, the artists not recommended and the tracks with a speed of
    /// their own, each taken back there.
    Marks,
    /// «Песни из альбомов → Альбом показывает»: «Автоматически», «Всегда весь альбом»,
    /// «Только мои песни».
    AlbumOpens,
    /// «Другие версии (инструментал, ремиксы)».
    AlbumVersions,
    /// The mixes of «Миксы для тебя» left out, with «Вернуть».
    HiddenMixes,
    /// «Разделы главной»: the order of Главная's sections and which show.
    HomeSections,
    /// The presets of an area, the first row of its group («Строго · Живо ·
    /// Максимум · Свои»).
    Preset(v::presets::Area),
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
    /// «Токен ListenBrainz».
    ListenbrainzToken,
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
    /// One line of «Откуда качать»: its switch and place.
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
    // Межсеть → API и MCP.
    /// «Использовать»: the local REST API on or off, «Открыть /docs».
    ApiEnabled,
    /// «Доступ»: this computer only, or the home network.
    ApiAccess,
    /// «Порт» of the API.
    ApiPort,
    /// «Токены»: «Новый токен» and the list of tokens.
    ApiTokens,
    // Межсеть → Устройства.
    /// «Связь с устройствами»: the mesh on or off.
    MeshEnabled,
    /// «Устройства»: this device and the paired ones, «Добавить устройство»,
    /// «Карта сети».
    MeshDevices,
    /// «Рядом»: unpaired instances on the home network, «Подключить».
    MeshNearby,
    /// «Оригиналы FLAC на ПК»: always, after one copy, after two.
    MeshKeepMasters,
    /// «Сохранять обработанные тексты, даже неудачные»: the word timing of
    /// a text that does not fit the recording is kept (never made again,
    /// passed on to the other devices).
    LyricsKeepMisfits,
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
pub const TAB_MUSIC: usize = 0;
pub const TAB_LYRICS: usize = 1;
pub const TAB_LOOK: usize = 2;
pub const TAB_SERVICES: usize = 3;
pub const TAB_DOWNLOADS: usize = 4;
pub const TAB_NET: usize = 5;
pub const TAB_SYSTEM: usize = 6;
/// Labels of the tab switch, in [`SettingId::tab`] order.
pub const TABS: [&str; 7] = ["Музыка", "Тексты", "Вид", "Сервисы", "Загрузки", "Межсеть", "Система"];

/// The tabs beside which the live stage stands (the lyrics and the looks).
pub fn stage_tab(tab: usize) -> bool {
    matches!(tab, TAB_LYRICS | TAB_LOOK)
}

/// When a row shows: in «Просто» (and «Подробно»), only in «Подробно», or
/// only in «Подробно» of the owner's own builds («Для разработчика»).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Simple,
    Deep,
    Dev,
}

/// «Запускать вместе с Windows», named as the user knows the system.
const AUTOSTART_LABEL: &str = if cfg!(windows) { "Запускать вместе с Windows" } else { "Запускать вместе с системой" };

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
    Turn,
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

const fn lang_choice_help(pref: nikgob_core::i18n::LangPref, loc: nikgob_core::i18n::Locale) -> &'static str {
    use nikgob_core::i18n::{LangPref, Locale};
    match (pref, loc) {
        (LangPref::Auto, Locale::Ru) => "Определяется по языку операционной системы.",
        (LangPref::Auto, Locale::En) => "Follows your operating system language.",
        (LangPref::Ru, Locale::Ru) => "Интерфейс на русском языке.",
        (LangPref::Ru, Locale::En) => "Russian interface.",
        (LangPref::En, Locale::Ru) => "Интерфейс на английском языке.",
        (LangPref::En, Locale::En) => "English interface.",
    }
}

const fn lang_choices(loc: nikgob_core::i18n::Locale) -> [Choice; nikgob_core::i18n::LangPref::ALL.len()] {
    use nikgob_core::i18n::LangPref;
    let mut out = [c!("", ""); LangPref::ALL.len()];
    let mut i = 0;
    while i < LangPref::ALL.len() {
        let pref = LangPref::ALL[i];
        out[i] = c!(pref.label_in(loc), lang_choice_help(pref, loc));
        i += 1;
    }
    out
}

static LANG_CHOICES_RU: [Choice; nikgob_core::i18n::LangPref::ALL.len()] = lang_choices(nikgob_core::i18n::Locale::Ru);
static LANG_CHOICES_EN: [Choice; nikgob_core::i18n::LangPref::ALL.len()] = lang_choices(nikgob_core::i18n::Locale::En);

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
        use v::presets::Area;
        match self {
            Preset(Area::Recs) => TAB_MUSIC,
            Preset(Area::Lyrics) => TAB_LYRICS,
            Preset(Area::Track) => TAB_LOOK,
            Preset(Area::Sources | Area::Seeding) => TAB_DOWNLOADS,
            Preset(Area::Load) => TAB_SYSTEM,
            OutputDevice | Gapless | Crossfade | Normalization | Equalizer | TrackSpeed | KeepTimbre | Autoplay | AutoplayWorld
            | SearchPlay | ListShuffle(_) | ShuffleRadio
            | RadioNoRepeat | RadioSource | MixesDownloadedOnly | RadioNewShare | NewShareBy(_) | MixesWithNew | RadioExplore
            | RadioUnheard | GraphUse | DislikeButton | DislikeSkips | DislikeSkipEverywhere | Marks | AlbumOpens | AlbumVersions
            | SoundNeural | SoundDevice | MixCardsWave => TAB_MUSIC,
            LyricsStyle | LyricsUnit | LyricsSmooth | LightSpeed | WaveJump | JumpSpeed | LyricsLift | LyricsScale | LyricsLead
            | LyricsDim | LyricsCascade | LyricsGlow | LyricsSoftEdges | LyricsSoftDepth | LyricsSoftStrength | LyricsSoftFade | LyricsGap
            | LyricsGapWaves | LyricsGapHeight | LyricsGapSize | LyricsCountdown | WaveFallback | LyricsReturn | LyricsGenius
            | SaveLyrics | LyricsCheck | LyricsWordsPlaying | LyricsLinesAuto | LyricsStreams | LyricsStreamKeep | LyricsRecognize
            | LyricsModels | LyricsKeepMisfits => TAB_LYRICS,
            Language | Theme | Scheme | Contrast | CoverShape | ShapeRange | ShapePick | CoverSpin | CoverDir | CoverPulse | CoverPauseMorph
            | CoverSpinSpeed | CoverPulseStrength | HomeSections
            | Ring | RingStyle | Ambient | AmbientStrength | BgShapes | BgCount | BgSize | BgOpacity
            | BgSpeed | BgReact | BgColors | BgHue | ProgressWave | NpVolume | NpVolumeHold | CoverClickPause | DblClickShape
            | TapWait | NpMotion | MixesProfiles | MixesEverything | MixesMoods | MixesOnRepeat | BackgroundCards | ProfileNames
            | HiddenMixes | AlbumDate | LikedAlbumsPlace | UiScale | RailCompact | ReducedMotion | Tooltips
            | PaneLyrics | PaneInfo | TrackMenu | PlayerButton(_) | NavRoot => TAB_LOOK,
            SpotifyEnabled | Spotify | SpotifyPersonal | SpotifyPlayback | SpotifySoloist | ImportSpotify | YouTubeEnabled
            | YouTube | YouTubeCookies | ImportYouTube | SoundCloudEnabled | SoundCloud | SoundCloudToken | ImportSoundCloud
            | DeezerEnabled | PushLikes | AlbumLike | SingleLike | PlaylistSync | ImportFiles | ImportSavedSongs | StreamCache => {
                TAB_SERVICES
            }
            DownloadSource(_) | LikeDownload | UpgradeBar | UpgradeEvery | OpusBitrate | UpgradeAuto | FakeFlac | FakeRestore
            | Soulseek | SoulseekShare | Prowlarr | NoSeed | SeedLimit | SeedDays | SeedRatio | DownloadLimit | UploadLimit
            | SoulseekUpload => TAB_DOWNLOADS,
            ApiEnabled | ApiAccess | ApiPort | ApiTokens | MeshEnabled | MeshDevices | MeshNearby | MeshKeepMasters => TAB_NET,
            UserName | Autostart | AutostartHidden | FpsLimit | TrayKeepWindow | AutoDownloads | StreamAnalysis | LyricsSeparate
            | LyricsDeep | LyricsWordsAll | ToastTime | DownloadToasts | LibraryDir | MastersDir | Inbox | Trash | ExportData
            // «Для разработчика».
            | LyricsUi | LyricsGpu | LyricsModelsAuto | MusicBrainz | AcoustId | LastfmKey | ListenbrainzToken
            | BackgroundCountsLess | ProfilesSplit | GraphServices | GraphOpenData | GraphPersonal | LibraryAsTaste
            | DislikeInRecs | DemoProfile | FirstRun => TAB_SYSTEM,
        }
    }

    /// When the row shows: «Просто» draws [`Level::Simple`], «Подробно»
    /// adds [`Level::Deep`], and in the owner's own builds [`Level::Dev`].
    ///
    /// «Просто» is what a user would look for by name: the presets;
    /// whether a thing seen or heard is there and which kind of it (the
    /// theme, the cover's shape, the ring, the lyrics' animation and its
    /// step); the accounts, sources and folders the player needs. «Подробно»
    /// adds how strong, how fast, how long and which way; what only matters
    /// in a rare case (a fallback, a gesture, a part of the window); and the
    /// technical rows (limits, the cache, the bitrate, the models, imports
    /// from files, sync).
    pub fn level(self) -> Level {
        use SettingId::*;
        match self {
            // Музыка.
            OutputDevice | Gapless | Crossfade | Normalization | Equalizer | TrackSpeed | Autoplay | ListShuffle(_) | DislikeButton
            | DislikeSkips | Marks | SoundNeural | MixCardsWave
            // Тексты: the animation and its step, the size and the lead, each
            // effect's switch, what lights up by itself.
            | LyricsStyle | LyricsUnit | LyricsScale | LyricsLead | LyricsCascade | LyricsGlow | LyricsSoftEdges | LyricsGap
            | LyricsCountdown | LyricsGenius | SaveLyrics | LyricsWordsPlaying | LyricsLinesAuto | LyricsStreams
            // Вид: the theme, each part of the track screen and which kind.
            | Language | Theme | Scheme | Contrast | CoverShape | ShapeRange | ShapePick | CoverSpin | CoverPulse | Ring
            | Ambient | BgShapes | ProgressWave | NpVolume | HomeSections | HiddenMixes | AlbumDate | LikedAlbumsPlace
            | UiScale | RailCompact | ReducedMotion
            // Сервисы: the accounts, their sound and their playlists.
            | SpotifyEnabled | Spotify | SpotifyPlayback | ImportSpotify | YouTubeEnabled | YouTube | ImportYouTube
            | SoundCloudEnabled | SoundCloud | ImportSoundCloud | DeezerEnabled | PushLikes
            // Загрузки.
            | DownloadSource(_) | LikeDownload | UpgradeBar | UpgradeEvery | Soulseek | Prowlarr
            // Система.
            | UserName | Autostart | AutostartHidden | DownloadToasts | LibraryDir | Inbox | Trash | Preset(_)
            // Межсеть: this device, the devices, the API's switch and the
            // tokens (a token is what every client needs).
            | ApiEnabled | ApiTokens | MeshDevices | MeshNearby => Level::Simple,
            LyricsUi | LyricsGpu | LyricsModelsAuto | MusicBrainz | AcoustId | LastfmKey | ListenbrainzToken
            | BackgroundCountsLess | ProfilesSplit | GraphServices | GraphOpenData | GraphPersonal | LibraryAsTaste
            | DislikeInRecs | DemoProfile | FirstRun => Level::Dev,
            KeepTimbre | AutoplayWorld | SearchPlay | ShuffleRadio | RadioNoRepeat | RadioSource | MixesDownloadedOnly | RadioNewShare
            | NewShareBy(_) | MixesWithNew | RadioExplore | RadioUnheard | GraphUse | DislikeSkipEverywhere | SoundDevice
            // Songs of albums: how an album only some songs of which are the
            // user's shows.
            | AlbumOpens | AlbumVersions
            // How strong, how fast, how long.
            | LyricsSmooth | LightSpeed | WaveJump | JumpSpeed | LyricsLift | LyricsDim | LyricsSoftDepth | LyricsSoftStrength
            | LyricsSoftFade | LyricsGapWaves | LyricsGapHeight | LyricsGapSize | LyricsReturn | CoverDir | CoverSpinSpeed
            | CoverPulseStrength | RingStyle | AmbientStrength | BgCount | BgSize
            | BgOpacity | BgSpeed | BgReact | BgColors | BgHue | NpVolumeHold | TapWait | ToastTime
            // Rare cases, gestures, parts of the window and of Главная.
            | WaveFallback | LyricsCheck | LyricsKeepMisfits | LyricsStreamKeep | LyricsRecognize | CoverPauseMorph | CoverClickPause
            | DblClickShape | NpMotion | MixesProfiles | MixesEverything | MixesMoods | MixesOnRepeat | BackgroundCards
            | ProfileNames | Tooltips | PaneLyrics | PaneInfo | TrackMenu | AlbumLike | SingleLike
            | PlayerButton(_) | NavRoot
            // Technical.
            | LyricsModels | SpotifyPersonal | SpotifySoloist | YouTubeCookies | SoundCloudToken | PlaylistSync
            | ImportFiles | ImportSavedSongs | StreamCache | OpusBitrate | UpgradeAuto | FakeFlac | FakeRestore
            | SoulseekShare | NoSeed | SeedLimit | SeedDays | SeedRatio | DownloadLimit | UploadLimit | SoulseekUpload
            | FpsLimit | TrayKeepWindow | AutoDownloads | StreamAnalysis | LyricsSeparate | LyricsDeep | LyricsWordsAll
            | MastersDir | ExportData | ApiAccess | ApiPort | MeshEnabled | MeshKeepMasters => Level::Deep,
        }
    }

    /// The nearest row a user sees, for a link to a [`Level::Dev`] row
    /// where developer rows do not show (and for a link to its whole group,
    /// which must not tint «Для разработчика»). Every other row is itself.
    pub fn user_neighbour(self) -> Self {
        use SettingId::*;
        match self {
            // «Настройки текста…» under the lyrics: the lyrics' own look.
            LyricsUi => LyricsStyle,
            LyricsGpu | LyricsModelsAuto => LyricsModels,
            MusicBrainz | AcoustId | DemoProfile | FirstRun => LibraryDir,
            LastfmKey | ListenbrainzToken | GraphServices | GraphOpenData | GraphPersonal | LibraryAsTaste => GraphUse,
            BackgroundCountsLess | ProfilesSplit => ProfileNames,
            DislikeInRecs => DislikeButton,
            other => other,
        }
    }

    /// The row's name in `loc`: the thing it changes, as the page shows it
    /// (never its value).
    pub fn label_in(self, loc: nikgob_core::i18n::Locale) -> &'static str {
        use SettingId::*;
        match self {
            Language => nikgob_core::i18n::settings_in(loc).language_label,
            Theme => "Тема",
            Contrast => "Контраст",
            Scheme => "Схема цвета",
            ProgressWave => "Волна на полосе прогресса",
            NpVolume => "Громкость на экране трека",
            NpVolumeHold => "Плашка громкости",
            CoverShape => "Форма обложки",
            ShapeRange => "Фигуры для случайной",
            ShapePick => "Фигура",
            CoverPauseMorph => "Менять форму на паузе",
            CoverSpin => "Вращение",
            CoverDir => "Направление",
            CoverPulse => "Пульсация",
            CoverSpinSpeed => "Скорость",
            CoverPulseStrength => "Сила",
            HomeSections => "Разделы главной",
            Preset(a) => preset_title(a),
            Ring => "Кольцо",
            RingStyle => "Вид",
            Ambient => "Свечение фона",
            AmbientStrength => "Яркость свечения",
            BgShapes => "Фигуры на фоне",
            BgCount => "Количество",
            BgSize | LyricsGapSize => "Размер",
            BgOpacity => "Яркость",
            BgSpeed => "Скорость",
            BgReact => "Реакция на музыку",
            BgColors => "Цвета",
            BgHue => "Свой цвет",
            LyricsStyle => "Анимация",
            WaveFallback => "Если нет времени слов",
            WaveJump => "Высота прыжка",
            JumpSpeed => "Скорость прыжка",
            LightSpeed => "Скорость подсветки",
            LyricsSmooth => "Плавность",
            LyricsDim => "Приглушение строк",
            LyricsScale => "Размер текста",
            LyricsLead => "Опережение подсветки",
            LyricsCascade => "Каскад строк",
            LyricsGlow => "Свечение",
            LyricsLift => "Подъём спетого",
            LyricsGap => "Индикатор паузы",
            LyricsGapWaves => "Длина волны",
            LyricsGapHeight => "Высота волн",
            LyricsCountdown => "Отсчёт 3-2-1",
            LyricsUnit => "Шаг подсветки",
            LyricsSoftEdges => "Мягкие края",
            LyricsSoftStrength => "Размытие у края",
            LyricsSoftDepth => "Глубина",
            LyricsSoftFade => "Затухание",
            UiScale => "Масштаб интерфейса",
            RailCompact => "Боковая панель всегда компактная",
            AlbumDate => "Дата альбома",
            PaneLyrics => "Текст песни",
            PaneInfo => "Источник и качество",
            TrackMenu => "Меню трека",
            PlayerButton(v::PlayerButton::Sound) => "Скорость и эквалайзер",
            PlayerButton(v::PlayerButton::Shuffle) => "Перемешивание",
            PlayerButton(v::PlayerButton::Repeat) => "Повтор",
            PlayerButton(v::PlayerButton::Like) => "Нравится",
            PlayerButton(v::PlayerButton::AddToPlaylist) => "В плейлист",
            PlayerButton(v::PlayerButton::Queue) => "Очередь",
            PlayerButton(v::PlayerButton::Lyrics) => "Текст песни",
            NavRoot => "Разделы открываются с начала",
            NpMotion => "Открытие экрана",
            ReducedMotion => "Меньше движения",
            Tooltips => "Подсказки при наведении",
            FpsLimit => "Лимит кадров",
            Autostart => AUTOSTART_LABEL,
            AutostartHidden => "Сразу в трей",
            TrayKeepWindow => "Быстрое открытие из трея",
            CoverClickPause => "Клик по обложке — пауза",
            DblClickShape => "Двойной клик — другая фигура",
            TapWait => "Пауза двойного клика",
            ToastTime => "На экране",
            SaveLyrics => "Сохранять текст в файл трека",
            LyricsReturn => "Возврат к строке",
            LyricsCheck => "Сверять текст с голосом",
            LyricsKeepMisfits => "Сохранять обработанные тексты, даже неудачные",
            LyricsLinesAuto => "Строки для простого текста",
            LyricsUi => "Где делать подсветку",
            LyricsWordsPlaying => "Слова для играющей песни",
            LyricsWordsAll => "Вся библиотека по словам",
            LyricsRecognize => "Расслышать текст",
            LyricsSeparate => "Отделять голос",
            LyricsDeep => "Глубокое создание субтитров",
            LyricsGpu => "Где считать",
            LyricsModelsAuto => "Скачивать модели сами",
            LyricsGenius => "Тексты с Genius",
            LyricsStreams => "Песни с YouTube",
            LyricsStreamKeep => "Сохранять с YouTube",
            LyricsModels => "Модели",
            ListShuffle(v::ListKind::Mix) => "Миксы",
            ListShuffle(v::ListKind::OwnPlaylist) => "Свои плейлисты",
            ListShuffle(v::ListKind::Album) => "Альбомы",
            ListShuffle(v::ListKind::ReadyPlaylist) => "Готовые плейлисты",
            ShuffleRadio => "Перемешивать радио и автовоспроизведение",
            OutputDevice => "Устройство вывода",
            Gapless => "Без пауз между треками",
            Crossfade => "Кроссфейд",
            Normalization => "Нормализация громкости",
            Equalizer => "Эквалайзер",
            TrackSpeed => "Скорость трека",
            KeepTimbre => "Сохранять тембр голоса",
            LibraryDir => "Папка музыки",
            MastersDir => "Архив FLAC",
            Inbox => "Inbox",
            Trash => "Корзина",
            ExportData => "Экспорт данных",
            OpusBitrate => "Битрейт Opus",
            FakeFlac => "Ненастоящий FLAC",
            FakeRestore => "Вернуть FLAC из Корзины",
            MusicBrainz => "Контакт MusicBrainz",
            AcoustId => "Ключ AcoustID",
            DemoProfile => "Демо-профиль",
            FirstRun => "Первый запуск",
            UserName => "Как к тебе обращаться",
            RadioNewShare => "Новое в радио и миксах",
            MixesDownloadedOnly => "Только из скачанного",
            Autoplay | NewShareBy(v::RadioKind::Autoplay) => "Автовоспроизведение",
            AutoplayWorld => "Продолжать мир после альбома",
            SearchPlay => "Трек из поиска",
            RadioSource => "Откуда",
            RadioNoRepeat => "Не повторять сыгранное",
            RadioExplore => "Случайные песни",
            RadioUnheard => "Неслышанные песни",
            SoundNeural => "Нейросетевой анализ звука",
            SoundDevice => "На чём считать анализ звука",
            MixCardsWave => "Карточки в «Миксах для тебя»",
            NewShareBy(v::RadioKind::Track) => "Радио по треку",
            NewShareBy(v::RadioKind::Artist) => "Радио по исполнителю",
            NewShareBy(v::RadioKind::Mix) => "Бесконечные миксы",
            MixesWithNew => "С новым в «Миксах для тебя»",
            BackgroundCountsLess => "Фон меньше влияет на рекомендации",
            ProfilesSplit => "Делить профили по звуку",
            ProfileNames => "Названия профилей",
            MixesProfiles => "Миксы профилей",
            MixesEverything => "Всё вперемешку",
            MixesMoods => "Спокойное и погромче",
            MixesOnRepeat => "На повторе",
            BackgroundCards => "Миксы для фона",
            DislikeButton => "Кнопка на экране трека",
            DislikeSkips => "Пропускать сразу",
            DislikeSkipEverywhere => "Пропускать везде",
            DislikeInRecs => "Дизлайк в рекомендациях",
            Marks => "Не нравится и скорость",
            AlbumOpens => "Альбом показывает",
            AlbumVersions => "Другие версии (инструментал, ремиксы)",
            HiddenMixes => "Скрытые миксы",
            GraphUse => "Похожее по слушателям",
            GraphServices => "Спрашивать YouTube Music и Deezer",
            GraphOpenData => "Спрашивать ListenBrainz и MusicBrainz",
            GraphPersonal => "Мои плейлисты и импорт истории",
            LibraryAsTaste => "Без истории — по библиотеке",
            LastfmKey => "Ключ Last.fm",
            ListenbrainzToken => "Токен ListenBrainz",
            SpotifyEnabled | YouTubeEnabled | DeezerEnabled | SoundCloudEnabled => "Использовать",
            Spotify | YouTube | SoundCloud => "Аккаунт",
            SpotifyPlayback => "Звук из Spotify",
            SpotifySoloist => "Spotify Soloist",
            SpotifyPersonal => "Свой ключ",
            YouTubeCookies => "Вход по cookies",
            SoundCloudToken => "Токен",
            ImportSpotify | ImportYouTube | ImportSoundCloud => "Плейлисты и история",
            LikeDownload => "Что качать по лайку",
            PushLikes => "Лайки обратно в сервисы",
            AlbumLike => "Лайк альбома",
            SingleLike => "Лайк песни из сингла",
            LikedAlbumsPlace => "Любимые альбомы",
            AutoDownloads => "Одновременных автозагрузок",
            DownloadToasts => "О загрузках",
            UpgradeEvery => "Искать копии лучше",
            UpgradeBar => "Хорошее качество",
            UpgradeAuto => "Замена файла",
            DownloadSource(k) => k.display_name(),
            PlaylistSync => "Синхронизация по умолчанию",
            ImportFiles => "Из файлов выгрузки",
            ImportSavedSongs => "Песни медиатеки YouTube Music",
            StreamCache => "Кэш стриминга",
            StreamAnalysis => "Анализ звука стримов",
            Prowlarr => "Prowlarr",
            Soulseek => "Soulseek",
            SoulseekShare => "Раздавать библиотеку",
            SoulseekUpload => "Отдача Soulseek",
            NoSeed => "Не раздавать",
            SeedLimit => "Лимит раздачи",
            SeedDays => "Срок раздачи",
            SeedRatio => "Предел отдачи",
            DownloadLimit => "Скорость загрузки",
            UploadLimit => "Отдача торрентов",
            ApiEnabled => "Использовать",
            ApiAccess => "Доступ",
            ApiPort => "Порт",
            ApiTokens => "Токены",
            MeshEnabled => "Связь с устройствами",
            MeshDevices => "Устройства",
            MeshNearby => "Рядом",
            MeshKeepMasters => "Оригиналы FLAC на ПК",
        }
    }

    /// The row's name in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(nikgob_core::i18n::locale())
    }

    /// Not tested by users yet: the row carries a «Бета» mark and its card
    /// says so.
    pub fn beta(self) -> bool {
        use SettingId::*;
        matches!(self, ImportSpotify | ImportYouTube | ImportSoundCloud | ImportFiles | LyricsRecognize)
    }

    /// One line on what the option changes in `loc`.
    pub fn help_in(self, loc: nikgob_core::i18n::Locale) -> &'static str {
        use SettingId::*;
        match self {
            Language => nikgob_core::i18n::settings_in(loc).language_help,
            Theme => "Светлый или тёмный фон приложения.",
            Contrast => "Насколько текст и кнопки выделяются на фоне. Выше — читать легче, но цвета резче.",
            Scheme => "Приложение окрашивается в цвета обложки играющего трека. Здесь выбирается, насколько ярко.",
            ProgressWave => "Пока играет музыка, полоска прогресса трека бежит волной. Выключи — будет ровной линией.",
            NpVolume => "Как менять громкость на экране трека: ползунок, всплывающая плашка или только кнопка.",
            NpVolumeHold => "Сколько плашка громкости остаётся на экране после того, как ты перестал крутить колесо или двигать ползунок.",
            CoverShape => "Обложка на экране трека может быть квадратной или вырезанной красивой фигурой.",
            ShapeRange => "Из этих фигур выбирается случайная для каждого трека. Нажми на фигуру, чтобы добавить или убрать её.",
            ShapePick => "Этой фигурой вырезается обложка, пока играет музыка.",
            CoverPauseMorph => "На паузе обложка плавно становится квадратом, при воспроизведении снова принимает форму. Выключи — форма остаётся и на паузе.",
            CoverSpin => "Край обложки медленно поворачивается, а сама картинка остаётся на месте.",
            CoverDir => "В какую сторону поворачивается край обложки.",
            CoverPulse => "Обложка слегка увеличивается в такт музыке, как будто дышит.",
            CoverSpinSpeed => "Сколько секунд занимает один оборот края обложки. Меньше — быстрее; на громких местах край чуть ускоряется.",
            CoverPulseStrength => "Насколько обложка увеличивается на ударе.",
            HomeSections => "Какие разделы есть на главной и в каком порядке. Стрелки двигают раздел, переключатель убирает его. Раздел, которому нечего показать, не виден и включённым.",
            Preset(v::presets::Area::Recs) => "Сколько новой музыки в радио, миксах и автовоспроизведении — одним выбором. «Свои» возвращает твои значения.",
            Preset(v::presets::Area::Lyrics) => "Готовые виды текста: как загорается и движется строка, которую поют, и как стоят остальные. Размер текста, опережение и пауза не меняются. «Свои» возвращает твои значения.",
            Preset(v::presets::Area::Track) => "Готовые наборы для экрана трека: обложка, кольцо и фон. Набор фигур, направления, скорости и цвета не меняются. «Свои» возвращает твои значения.",
            Preset(v::presets::Area::Sources) => "Какие источники включены. Порядок не меняется. «Свои» возвращает твои переключатели.",
            Preset(v::presets::Area::Seeding) => "Сколько и как долго раздавать скачанное торрентами. «Свои» возвращает твои значения.",
            Preset(v::presets::Area::Load) => "Насколько плеер нагружает компьютер: кадры, окно в трее, загрузки и работа с голосом. «Свои» возвращает твои значения.",
            Ring => "Пока играет музыка, вокруг обложки-фигуры кружится кольцо из точек и подскакивает в такт. Вокруг квадратной обложки и на паузе его нет.",
            RingStyle => "Как выглядит кольцо вокруг обложки.",
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
            WaveJump => "Как высоко подпрыгивают слоги, буквы или слова в анимации «Волна»: взлетают пружиной чуть выше и опускаются с лёгким отскоком. 0 — они только загораются.",
            JumpSpeed => "Как быстро буква или слово в «Волне» взлетает, когда его начинают петь, и встаёт на место. Больше — резче и короче.",
            LightSpeed => "Как быстро загорается буква или слово, когда его поют: от плавного появления до мгновенного. В караоке — насколько мягкий край у подсветки.",
            LyricsSmooth => "Подсветка идёт по времени слогов и слов. Чем плавнее, тем ровнее она скользит по строке: короткий слог не проскакивает, а протяжный всё равно тянется дольше. В «Волне» соседние буквы поднимаются вместе с поющейся. 0% — строго по времени.",
            LyricsDim => "Насколько бледнее остальные строки по сравнению с той, что поют сейчас.",
            LyricsScale => "Размер букв в тексте песни.",
            LyricsLead => "На сколько подсветка опережает голос. Глаз читает чуть раньше, чем слышит, поэтому подсветка, которая загорается немного заранее, кажется точной. Если она торопится — уменьшите, если отстаёт — увеличьте. Перемотка и время в тексте не меняются.",
            LyricsCascade => "При прокрутке дальние строки догоняют с небольшой задержкой — текст движется мягко, волной.",
            LyricsGlow => "Уже спетая часть строки слегка светится.",
            LyricsLift => "В караоке спетые слова мягко приподнимаются над строкой, пока подсветка проходит по ним. 0 % — строка стоит ровно.",
            LyricsGap => "Когда в песне долго нет слов, между строками крутится кружок ожидания. Здесь выбирается его вид.",
            LyricsGapWaves => "Сколько волн на кружке ожидания. Больше — волны мельче и чаще.",
            LyricsGapHeight => "Насколько высокие волны на кружке ожидания.",
            LyricsGapSize => "Насколько большой кружок ожидания между строками.",
            LyricsCountdown => "За три секунды до новой строки кружок ожидания сменяется отсчётом 3, 2, 1.",
            LyricsUnit => "Что подсвечивается по очереди в любой анимации: каждая буква, каждый слог или каждое слово целиком. Слова через дефис подсвечиваются по частям.",
            LyricsSoftEdges => "Строки у верхнего и нижнего края текста плавно тают, а не обрезаются.",
            LyricsSoftStrength => "Насколько расплываются буквы у самого края, где строка тает. Размытие нарастает только к краю: середина текста всегда чёткая.",
            LyricsSoftDepth => "Какая часть высоты текста сверху и снизу занята мягкими краями. Текущая строка всегда остаётся в чёткой середине.",
            LyricsSoftFade => "Как рано строка у края становится прозрачной: меньше — тает только у самого края, больше — начинает таять раньше.",
            UiScale => "Всё окно крупнее или мельче, поверх масштаба системы. Ещё Ctrl+= и Ctrl+- (на шаг), Ctrl+0 (100 %).",
            RailCompact => "Левая панель всегда узкая: только значки разделов, без подписей и кнопки раскрытия.",
            AlbumDate => "Что показывать о дате выхода на странице альбома. Если день или месяц неизвестны, показывается то, что есть.",
            PaneLyrics => "Справа от страницы открывается текст песни: кнопка в плеере внизу и пункт в меню трека. На экране трека текст есть всегда.",
            PaneInfo => "Справа от страницы открываются источник и качество трека: пункт «Источник и качество» в меню трека. На экране трека они есть всегда.",
            TrackMenu => "Где стоит каждый пункт меню трека: в самом меню, в подменю «Файл» или скрыт. «Сбросить» возвращает меню как было.",
            PlayerButton(v::PlayerButton::Sound) => "Кнопка скорости трека и эквалайзера на экране трека. Без неё окно звука открывается клавишей E и из меню трека.",
            PlayerButton(v::PlayerButton::Shuffle) => "Кнопка перемешивания на экране трека и в нижней панели. Перемешивание работает и без неё.",
            PlayerButton(v::PlayerButton::Repeat) => "Кнопка повтора на экране трека и в нижней панели.",
            PlayerButton(v::PlayerButton::Like) => "Сердце на экране трека и в нижней панели. Ctrl+L и меню трека работают и без него.",
            PlayerButton(v::PlayerButton::AddToPlaylist) => "Кнопка «В плейлист…» на экране трека. Это же есть в меню трека.",
            PlayerButton(v::PlayerButton::Queue) => "Кнопка очереди на экране трека и в нижней панели. Очередь открывается и клавишей Q.",
            PlayerButton(v::PlayerButton::Lyrics) => "Кнопка текста песни на экране трека и в нижней панели. Текст открывается и клавишей L.",
            NavRoot => "«Главная» и «Библиотека» в боковой панели открывают начало раздела. Выключи — откроется страница раздела, на которой ты был.",
            NpMotion => "Как экран трека открывается из плеера внизу и закрывается обратно.",
            ReducedMotion => "Убирает подпрыгивания, вращения и волны во всём приложении.",
            Tooltips => "Короткие подсказки, когда задерживаешь курсор на кнопке.",
            FpsLimit => "Как часто перерисовывается экран. Меньше — тише вентилятор и дольше работает батарея.",
            Autostart => AUTOSTART_HELP,
            TrayKeepWindow => "Крестик прячет окно в трей, но не закрывает его совсем: по клику на значок оно появляется сразу, там же, где было. Пока окно спрятано, оно ничего не рисует и не тратит процессор, но держит в памяти свои картинки и шрифты. Выключи — окно будет закрываться полностью и собираться заново при открытии: памяти меньше, открывается чуть дольше.",
            AutostartHidden => AUTOSTART_HIDDEN_HELP,
            CoverClickPause => "Нажатие на обложку на экране трека ставит музыку на паузу и снова запускает. Клавиши там же: L — текст рядом с обложкой, Q — очередь, Esc — назад к обложке.",
            DblClickShape => "Двойной клик по экрану трека меняет фигуру обложки.",
            TapWait => "Чтобы отличить одиночный клик (пауза) от двойного (другая фигура), плеер чуть ждёт второго клика. Меньше — пауза срабатывает быстрее, но двойной клик нужно делать быстрее. 0 — пауза мгновенная, а второй клик снимает её обратно и меняет фигуру.",
            ToastTime => "Сколько держится уведомление внизу окна, пока не уйдёт само. Пока на нём курсор, время стоит.",
            SaveLyrics => "Найденный в интернете текст песни сохраняется прямо в файл трека и потом показывается без интернета.",
            LyricsReturn => "Если ты сам прокрутил текст, через столько секунд он вернётся к строке, которую поют.",
            LyricsCheck => "Плеер слушает голос играющей песни и проверяет, её ли это текст. Текст другой песни или другой версии уступает подходящему, а строки, которые спешат или опаздывают, встают по голосу.",
            LyricsKeepMisfits => "Подсветка слов остаётся в библиотеке, даже если текст не совпал с голосом: её не нужно делать заново, и она уходит на твои другие устройства. Показывается такой текст как раньше — без подсветки по словам.",
            LyricsUi => "Источник текста и то, как его видеть (по словам, по строкам или без подсветки), всегда выбираются в маленьком меню у кнопки под текстом. Здесь выбирается, где делать подсветку по строкам или словам: прямо в этом меню, в окне поверх плеера или на отдельном экране «Текст песни».",
            LyricsLinesAuto => "Если ни у одного источника нет текста со временем строк, плеер сам ставит строки простого текста по голосу играющей песни, и текст идёт вместе с песней.",
            LyricsWordsPlaying => "Плеер сам расставляет слова по голосу в песне, которая играет, и подсветка идёт по словам даже там, где источник дал только строки или простой текст.",
            LyricsWordsAll => "Плеер заранее расставляет слова по голосу во всех песнях с файлом, понемногу в фоне, пока он открыт. На большой библиотеке это часы работы видеокарты. Сделанное хранится в библиотеке и не делается заново.",
            LyricsRecognize => "Если текста нет нигде, плеер пробует расслышать слова сам. Работает медленно и ошибается, особенно в быстром рэпе и сильно обработанном голосе.",
            LyricsSeparate => "Перед сверкой голос отделяется от музыки: слова ложатся точнее. На процессоре это долго, около двух минут на песню.",
            LyricsDeep => "Подсветка делается моделями крупнее: голос отделяется чище, слова слушаются дважды. Модели скачиваются при первом включении; на видеокарте песня занимает до минуты, на процессоре — несколько минут.",
            LyricsGpu => "Где считают модели. На видеокарте (через DirectML) в десятки раз быстрее; если она не справится, плеер сам перейдёт на процессор.",
            LyricsModelsAuto => "Нужные модели скачиваются сами, когда понадобятся. Выключено — только вручную, в «Моделях».",
            LyricsGenius => "Если подходящего текста со временем нет, плеер берёт простой текст с genius.com. Найденный текст хранится в библиотеке, второй раз его не ищут.",
            LyricsStreams => "Подсветку по словам можно сделать и для песни, которая играет с YouTube: плеер на время скачивает её звук, слушает голос и удаляет звук. Только для песен, где ясно, кто и что поёт.",
            LyricsStreamKeep => "Что «Сохранить текст» кладёт в папку исполнителя в библиотеке для песни без файла.",
            LyricsModels => "Модели, которыми плеер слушает голос и ставит строки и слова. Хранятся рядом с библиотекой; для песни скачиваются только нужные.",
            OutputDevice => "Через что играет звук: колонки, наушники и т. д. Переключается без паузы.",
            ListShuffle(v::ListKind::Mix) => "Как играет микс главной, «Микс дня», смесь профилей и «Слушать микс» исполнителя. «Перемешать» всегда перемешивает.",
            ListShuffle(v::ListKind::OwnPlaylist) => "Как играют плейлисты, сделанные здесь, и «Любимые». «Перемешать» всегда перемешивает.",
            ListShuffle(v::ListKind::Album) => "Как играет альбом — из библиотеки или из сервиса. «Перемешать» всегда перемешивает.",
            ListShuffle(v::ListKind::ReadyPlaylist) => "Как играют плейлисты сервисов: открытые там, подключённые или перенесённые оттуда. «Перемешать» всегда перемешивает.",
            ShuffleRadio => "При включённом перемешивании радио, автовоспроизведение и дейлист тоже играют вперемешку. Выключи — они идут в своём порядке.",
            Gapless => "Треки альбома идут друг за другом без тишины, как на диске.",
            Crossfade => "Конец трека плавно затихает, пока начинается следующий. 0 — без кроссфейда.",
            Normalization => "Все треки звучат примерно одинаково громко, не нужно крутить громкость.",
            Equalizer => "Звук под наушники или колонки: готовая кривая или своя, десять полос. «Настроить» открывает кривую; она же в меню играющего трека и по клавише E.",
            TrackSpeed => {
                "В меню трека есть «Скорость» (у играющего — ещё кнопка на экране плеера и клавиша S): быстрее, nightcore, замедленно, замедленно с эхом или своя, от ×0.5 до ×2. Выбор запоминается для трека. Выключено: всё играет как записано, выбор остаётся."
            }
            KeepTimbre => {
                "На другой скорости высота меняется вместе с темпом, а голос остаётся похож на себя: не писклявый на быстрой и не глухой на медленной."
            }

            LibraryDir => "Папка на диске, где лежит твоя музыка. Добавленное раскладывается так: Исполнитель/Год - Альбом/01 - Трек.opus.",
            MastersDir => "Сюда откладываются исходные файлы FLAC после сжатия в Opus, чтобы ничего не потерять.",
            Inbox => "Положи файлы в эту папку — и они сами добавятся в библиотеку.",
            Trash => "Включена — удалённое 30 дней лежит в Библиотека → Корзина, и его можно вернуть. Выключена — удаляется сразу.",
            ExportData => "Лайки и плейлисты хранятся в обычных файлах рядом с музыкой, их легко сохранить.",
            OpusBitrate => "Качество звука при сжатии новых треков. Выше — звук лучше, но файлы больше.",
            FakeFlac => "«Ненастоящий» FLAC — файл, сделанный из MP3: высокое качество в нём только на бумаге. Что делать с такими файлами при добавлении в библиотеку.",
            FakeRestore => "Возвращает из Корзины все FLAC, которые стали MP3, и снова играет их; MP3 уходят в Корзину. Лайки, прослушивания, плейлисты и тексты остаются.",
            MusicBrainz => "Почта или сайт для сервиса MusicBrainz, откуда берутся названия и обложки. Сервис просит его указать.",
            AcoustId => "Ключ сервиса AcoustID: он узнаёт трек по звуку, если у файла нет названий.",
            DemoProfile => "Отдельный плеер со своей библиотекой популярной музыки, настройками и входами в сервисы — для видео и проверки интерфейса. Работает рядом с этим; твоя библиотека и аккаунты в нём не видны. «Сбросить» стирает только его папку и собирает её заново.",
            FirstRun => "Страницы первого запуска: откуда брать музыку, хранить ли её, вход в сервисы, вид текста. Выбранное там меняет те же настройки, что и здесь.",
            UserName => "Имя в приветствии на главной: «Добрый вечер, Ник». Пустое поле — приветствие без имени. Сохраняется, когда уходишь из поля или жмёшь Enter.",
            Prowlarr => "Адрес и ключ программы Prowlarr — через неё ищутся раздачи с музыкой.",
            Soulseek => "Свой аккаунт в сети Soulseek: новый ник и пароль заводятся сами при первом входе. «В плеере» — клиент работает внутри плеера в фоне; «На сервере» — утилита nikgob-slsk на сервере или Raspberry Pi, плеер забирает у неё скачанное. Хранится в settings.toml.",
            SoulseekShare => "Другие люди в Soulseek могут скачивать у тебя треки из папки библиотеки (только чтение). Сеть держится на обмене: кто раздаёт, тому охотнее отдают.",
            SoulseekUpload => "Максимальная скорость, с которой Soulseek отдаёт треки другим. До упора вправо — без лимита.",
            NoSeed => "Скачанные треки сразу перестают раздаваться другим.",
            SeedLimit => "Сколько всего можно отдать другим, после чего раздача остановится. До упора влево — без лимита: раздача не останавливается по объёму.",
            SeedDays => "Сколько дней раздавать скачанное, потом раздача удаляется.",
            SeedRatio => "Раздача остановится, когда ты отдашь во столько раз больше, чем скачал. До упора влево — без предела.",
            DownloadLimit => "Максимальная скорость скачивания торрентов, чтобы не занимать весь интернет. До упора вправо — без лимита.",
            UploadLimit => "Максимальная скорость, с которой торренты отдают скачанное другим. До упора вправо — без лимита.",
            ApiEnabled => "Локальный API плеера: им управляют голосовой ассистент, скрипты и ИИ-агенты — включают музыку, ставят паузу, меняют громкость и очередь, ищут в библиотеке. Каждому нужен свой токен. «Открыть /docs» — описание всех запросов с примерами; кнопка работает, когда API действительно слушает порт.",
            ApiAccess => "«Этот компьютер» — к API могут обратиться только программы на этом компьютере. «Домашняя сеть» — ещё и другие устройства дома: колонка с ассистентом, телефон, второй компьютер; без токена не пустит никого. Windows при первом открытии в сеть спросит, разрешить ли плееру входящие подключения, — разреши для частных сетей. Адрес, заданный в settings.toml (как на Raspberry Pi), показан рядом.",
            ApiPort => "Порт, на котором слушает API, от 1024 до 65535; по умолчанию 47210. Меняй, только если он занят другой программой, и не забудь поменять его в настройках ассистента.",
            MeshEnabled => "Плеер сам находит другие твои устройства в домашней сети — компьютер, Raspberry Pi с nikgobd, потом телефон и телевизор — и держит с ними связь. Выключи — он не виден в сети и никого не видит; привязки при этом сохраняются.",
            MeshDevices => "Первая строка — этот компьютер, ниже — привязанные устройства: они доверяют друг другу, видят, что где играет, и могут управлять друг другом. Форма значка — какое это устройство, заливка — в сети ли оно; пока там играет музыка, значок крутится. Число — сколько места занимает музыка на устройстве. Чип справа — что устройство хранит: полную копию, библиотеку без оригиналов, только избранное (до выбранного объёма) или ничего. «Гость» — устройство может играть и читать, но ничего твоего не меняет. Пока на устройство едут файлы, по нижнему краю строки бежит волна. «Добавить устройство» даёт одноразовое приглашение на 15 минут: QR-код и команду установки для Linux и Raspberry Pi. «Забыть» отключает устройство на всех сразу; передумал — «Отменить». «Карта сети» показывает, как устройства связаны и что между ними идёт. Windows при первом включении спросит, разрешить ли плееру связь в сети, — разреши для частных сетей, иначе устройства друг друга не увидят. Пока этот компьютер ни к чему не привязан, в его строке стоит код из шести цифр: его вводят в «Подключить» на другом устройстве.",
            MeshKeepMasters => "Оригиналы FLAC занимают больше всего места. «Всегда» — они остаются на этом компьютере, что бы ни хранили другие устройства. «После 1 копии» и «После 2 копий» — компьютер может отпустить оригинал, когда его проверенная копия лежит на одном или двух других устройствах (например, на Raspberry Pi с полной копией); файл уходит в корзину, а не удаляется сразу. Пока у какого-то устройства единственные копии, «Забыть» его сначала спросит.",
            MeshNearby => "Плееры в домашней сети, ещё не привязанные к твоим устройствам. «Подключить» спросит код, который показывает то устройство; на Raspberry Pi его печатает команда nikgobd pair-code. После пяти неверных кодов устройство покажет новый.",
            ApiTokens => "Токен — пароль для одной программы. Он показывается один раз, сразу после создания: скопируй его тогда, потом плеер хранит только его отпечаток и показать снова не сможет. Права: «Управление» — плеер, очередь, громкость, приглушение, вывод звука, поиск и чтение библиотеки; «Полный» — то же и сами файлы песен и обложки; «Только чтение» — смотреть и искать. «Кто включает» — кто будет пользоваться токеном: ты, ассистент, агент или гость. «Отозвать» отключает токен сразу; передумал — «Отменить».",
            Spotify => "Вход в аккаунт Spotify через браузер со своим ключом: поиск, лайки, плейлисты и история. Нужен Premium — без него Spotify не подключить, а YouTube Music работает и так. Пароль приложение не видит.",
            SpotifyEnabled => "Выключи — Spotify не используется нигде: ни в поиске, ни для звука, ни для лайков. Вход при этом не забывается.",
            SpotifyPlayback => "Отдельное разрешение на звук из Spotify (второе окно согласия). Пока его нет, те же треки играют с YouTube Music — в плеере это видно по подписи «YouTube».",
            SpotifySoloist => "Spotify Soloist — официальный плеер Spotify без окна. Скачанный, со своим ключом и выбранный один раз в приложении Spotify, он играет треки Spotify; без него они играют как раньше. Скачивается только с сервера Spotify и только с твоего согласия. Ключ у каждого свой, на developer.spotify.com/dashboard/soloist, нужен Premium. Кроссфейд и «без пауз» к его трекам не применяются.",
            SpotifyPersonal => "Ключ — это Client ID своего приложения на developer.spotify.com, со своей квотой запросов. Spotify пускает такое приложение, только пока у владельца Premium. Заводится один раз; хранится в settings.toml.",
            YouTube => "Вход нужен только для лайков, плейлистов и истории; поиск и звук работают и без него. «Войти через Google» открывает окно со страницей входа Google: пароль вводится только там, приложение его не видит и берёт лишь cookies сессии. Окно запоминает вход, так что повторный вход — в один клик; «Выйти» забывает и его. Если Google не пускает во встроенном окне — «Вставить cookies…».",
            YouTubeEnabled => "Выключи — YouTube Music не используется: ни в поиске, ни как запасной звук для треков Spotify.",
            DeezerEnabled => "Deezer — открытый каталог: поиск, альбомы и дискографии исполнителей без аккаунта и без ключа. Своего звука у него нет: трек Deezer играет с YouTube Music (по ISRC, иначе по исполнителю, названию и длительности). Лайк трека Deezer остаётся здесь — в Deezer ничего не уходит. Выключи — поиск и дискографии обойдутся YouTube Music и Spotify.",
            SoundCloud => "Вход нужен для лайков, плейлистов, истории и подписок; поиск и звук работают и без него. «Войти» открывает страницу входа SoundCloud в окне приложения: пароль вводится только там, приложение берёт лишь токен сессии и хранит его в системном хранилище паролей. Треки, которые SoundCloud отдаёт только в защищённом виде или только 30-секундным отрывком, играют с YouTube Music.",
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
            DownloadSource(v::SourceKey::Torrents) => "Раздачи, которые находит Prowlarr. Из альбома или дискографии берётся только нужный трек. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Khinsider) => "Саундтреки игр с downloads.khinsider.com, часто во FLAC. Спрашивается только для саундтреков: альбом так и называется или песня написана для игры. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Soulseek) => "Отдельные треки от других людей в сети Soulseek, часто во FLAC. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Qobuz) => "Каталог Qobuz во FLAC (до Hi-Res) через открытый сервис-посредник, без своего аккаунта. Такие сервисы то работают, то нет: если не отвечает, плеер просто попробует позже. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Tidal) => "Каталог Tidal во FLAC через открытые сервисы-посредники, без своего аккаунта. Их часто блокируют: если не отвечают, плеер попробует позже. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Squidify) => "Открытый сервер squidify.org: саундтреки аниме, игр и кино во FLAC, гостевой вход. Файлы берутся по одному, с паузами. Можно подключить свой сервер Subsonic или Navidrome: адрес и вход — в settings.toml, раздел [sources] (subsonic_url, subsonic_user, subsonic_password). Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Archive) => "Загрузки людей на archive.org: оцифровки дисков, редкие и старые альбомы, часто во FLAC. Названия там свободные, поэтому берётся только то, что сходится с заявкой: исполнитель, название и длина трека, у альбома — число треков. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Bandcamp) => "Релизы, которые музыканты на Bandcamp отдают бесплатно или за сколько не жалко, во FLAC. Платные и те, что просят почту, не берутся. Трек, который продаётся отдельно, берётся из бесплатного альбома. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Soundcloud) => "Оригиналы треков, которые авторы на SoundCloud разрешили скачивать: часто WAV или AIFF, бывает MP3. Нужен вход в SoundCloud в «Сервисах», без него отсюда ничего не берётся. Формат узнаётся по самому файлу, а не по названию. Только треки. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            DownloadSource(v::SourceKey::Dogmazic) => "Свободная музыка с play.dogmazic.net под лицензиями Creative Commons, без аккаунта. В основном MP3, иногда Ogg и WAV: формат узнаётся по самому файлу. Только треки; со своим бесплатным входом Dogmazic — и альбомы: dogmazic_user и dogmazic_password в settings.toml, раздел [sources]. Файлы берутся по одному, с паузами. Сначала берётся FLAC, где бы он ни был; при равном качестве — источник выше. Копия с YouTube — только когда нигде нет.",
            PlaylistSync => "Как новый плейлист, связанный с сервисом, следит за ним. У каждого плейлиста это можно поменять в его меню ⋮.",
            ImportSpotify => "Забирает из Spotify свои плейлисты и недавние прослушивания. Лайки приходят сами сразу после входа. Плейлисты появятся в библиотеке; скачивается только то, что скачаешь сам.",
            ImportYouTube => "То же из YouTube Music: плейлисты и история. Лайки приходят сами после входа.",
            RadioNewShare => "Радио по треку или исполнителю и бесконечные миксы (настроения, «Твои миксы», «Новое для тебя») подмешивают музыку, которой у тебя ещё нет: похожее из YouTube Music и Deezer. Здесь — сколько. «Твой микс» на главной собирается только из твоего. Пока включено «Только из скачанного», не действует: нового нет нигде.",
            Autoplay => "Когда в очереди играет последний трек, она не кончается: следом сами добавляются похожие треки, как в радио по этому треку, и дальше дописываются по мере игры. Повтор не мешает: при включённом повторе очередь просто начинается заново. Откуда берутся треки — как у радио: только скачанное, вся библиотека или с новым из сервисов (это же переключается прямо в очереди).",
            SearchPlay => "Что играет после трека, включённого из результатов поиска. Найденные альбом, плейлист и исполнитель играют своим списком в любом случае.",
            AutoplayWorld => "Если в очереди был альбом, после него автовоспроизведение сначала играет следующие треки его мира по порядку альбомов — саундтрека франшизы или дискографии исполнителя, что ещё не слышал, — и только когда мир кончится, похожее, как радио. Выключи — сразу похожее.",
            RadioSource => "Откуда радио, бесконечные миксы и автовоспроизведение берут треки.",
            MixesDownloadedOnly => "Всё, что подбирается для тебя — «Твой микс», дейлист, плитки главной, настроения, «Твои миксы», миксы исполнителей и радио, — берёт только треки, у которых есть файл: ни стримов, ни новой музыки из сервисов. Плитки, которым не хватило таких треков, пропадают, «Ещё не скачано» не показывается. Главная пересобирается сразу. «Новое для тебя» остаётся: это отдельная полка новинок.",
            StreamAnalysis => "Лайкнутые и плейлистные треки без файла тоже анализируются: их звук по одному тихо скачивается с YouTube Music в память (несколько мегабайт), анализируется и сразу выбрасывается. Не чаще раза в 20 секунд и не больше 300 треков в день; трек, сыгравший из стрима, анализируется из кэша без лишнего скачивания. Выключи — стримы останутся без анализа, а миксы будут подбирать их по исполнителю и истории.",
            StreamCache => "Недавно игравшие треки хранятся на диске, и «Назад» или повтор не качают их заново. 0 — не хранить. YouTube — файлы как есть, Spotify — только в зашифрованном виде, как у официального клиента. Лимит у каждого сервиса свой; старое удаляется само.",
            ImportFiles => "Файлы выгрузки — вся история прослушиваний за годы, папкой или .zip. Spotify: Аккаунт → Конфиденциальность → «Расширенная история стриминга» (готовится до 30 дней). Google Takeout: YouTube и YouTube Music. История учит рекомендации и не добавляет в библиотеку ни треков, ни альбомов; само ничего не скачивается.",
            ImportSavedSongs => "Песни, сохранённые в медиатеке YouTube Music (в Takeout — файл «music library songs»), учат рекомендации, что тебе нравится: без лайков, без загрузок, в «Библиотеке» и «Любимых» их не будет. Выключи — этот файл при импорте пропускается.",
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
            RadioExplore => "Как часто радио и автовоспроизведение ставят новую песню наугад, не зная, похожа ли она по звуку на то, что играет. Похожее новое это не ограничивает.",
            RadioUnheard => "Сколько в радио и автовоспроизведении песен, которых ты ещё не слышал: из твоей музыки и новых. Когда таких не остаётся, играет как обычно.",
            SoundNeural => "Нейросеть слушает песни и различает жанр, стиль и вокал — то, чего не слышит обычный анализ, поэтому похожее подбирается точнее. Модель (18 МБ и основа для моделей, если её ещё нет) скачивается один раз, когда выберешь режим. Работает в фоне по очереди с другими тяжёлыми задачами, ждёт, пока идёт игра, и выгружается из памяти, когда не нужна.",
            SoundDevice => "Где нейросеть считает анализ звука. Процессор берёт меньше памяти, видеокарта считает быстрее. «Автоматически» выбирает сам перед каждой порцией треков: процессор, пока он свободен, видеокарту, когда процессор занят, и снова процессор, когда занята видеокарта (игра, полная память).",
            MixCardsWave => "Что делает нажатие на карточку микса на главной: запускает «Мою волну» с вайбом этого микса — бесконечно, с новой музыкой и подстройкой под пропуски, — открывает его готовый список песен, как раньше, или сразу его играет. Список всегда есть в ⋮ → «Список треков».",
            BackgroundCards => "Ровная музыка без слов для фона. Появляется, только если ты так слушаешь.",
            DislikeButton => "Кнопка в конце ряда кнопок на экране трека (♡ — в его начале). «Не нравится» остаётся и в меню ⋮ любого трека.",
            DislikeSkips => "Трек, которому поставили «Не нравится», сразу пропускается.",
            DislikeSkipEverywhere => "Треки с «Не нравится» и исполнители, которых не рекомендовать, пропускаются и в твоих альбомах и плейлистах. Выключено — только в миксах, радио и автовоспроизведении; трек, выбранный тобой, играет всегда.",
            DislikeInRecs => "«Не нравится» чуть опускает и самые похожие по звуку треки. Выключено — уходит только сам трек. Исполнителя дизлайк не трогает.",
            Marks => "Треки с «Не нравится», исполнители, которых не рекомендовать, и треки со своей скоростью. Там же их можно снять или сбросить.",
            AlbumOpens => "Что показывает страница альбома, у которого у тебя только часть песен, и что играет «Слушать». «Мои треки» и «Весь альбом» на странице — в один клик.",
            AlbumVersions => "Инструментал, off vocal, караоке и ремиксы песен того же альбома, которые ты не выбирал, стоят на странице альбома отдельно, после остальных, и не играют с альбомом. Включено — играют все, кроме караоке. У каждого альбома свой переключатель над этими песнями. Версия, которую ты лайкнул или добавил сам, — твоя песня и играет всегда.",
            HiddenMixes => "Миксы, убранные с главной через ⋮ → «Не предлагать этот микс». «Вернуть» снова показывает микс.",
            GraphUse => "Радио, миксы и новое учитывают, что другие люди слушают вместе: радио YouTube Music, похожих исполнителей Deezer, ListenBrainz и Last.fm. Так рекомендации работают и без истории, по одним твоим файлам. Выключи — остаются звук, твоя история и тип песни.",
            GraphServices => "Понемногу и в фоне спрашивать у YouTube Music радио твоих треков, а у YouTube Music и Deezer — похожих исполнителей. Ответы хранятся у тебя и работают без сети.",
            GraphOpenData => "Понемногу и в фоне спрашивать открытые данные: похожих исполнителей и треки в ListenBrainz, жанры исполнителей в MusicBrainz. Без ключей и аккаунтов.",
            GraphPersonal => "Треки, которые стоят рядом в твоих плейлистах или шли подряд в загруженной истории YouTube Music или Spotify, считаются похожими.",
            LibraryAsTaste => "Пока истории мало, вкус берётся из самой библиотеки: профили и миксы есть с первого запуска, а треки, вокруг которых собрана коллекция, стоят выше. Чем больше ты слушаешь, тем меньше это значит.",
            LastfmKey => "Свой бесплатный ключ Last.fm API: с ним похожие треки и исполнители спрашиваются и у Last.fm. Хранится в settings.toml. Пусто — Last.fm не спрашивается.",
            ListenbrainzToken => "Свой бесплатный токен ListenBrainz (listenbrainz.org → Settings → User token): с ним популярность треков исполнителя спрашивается и у ListenBrainz. Хранится в settings.toml. Пусто — популярность берётся из топа Deezer и, с ключом, Last.fm.",
        }
    }

    /// One line on what the option changes in the active process locale.
    pub fn help(self) -> &'static str {
        self.help_in(nikgob_core::i18n::locale())
    }

    /// The options of a choice row (segmented buttons or a menu) in `loc`, in
    /// order; empty for every other row.
    pub fn choices_in(self, loc: nikgob_core::i18n::Locale) -> &'static [Choice] {
        use v::presets::Area;
        match self {
            Self::Language => match loc {
                nikgob_core::i18n::Locale::Ru => &LANG_CHOICES_RU,
                nikgob_core::i18n::Locale::En => &LANG_CHOICES_EN,
            },
            Self::Preset(Area::Recs) => &[
                c!("Только моё", "Радио, миксы и автовоспроизведение берут только твою музыку."),
                c!("Сбалансировано", "Немного новой музыки в радио и миксах, изредка песня наугад."),
                c!("Больше нового", "Больше новой и неслышанной музыки, песни наугад чаще."),
            ],
            Self::Preset(Area::Lyrics) => &[
                c!("Караоке", "Спетое заливается по слогам мягким краем и чуть приподнимается, остальные строки приглушены, края текста мягкие."),
                c!("Спокойно", "Строка, которую поют, загорается целиком, внутри ничего не движется. Удобно читать."),
                c!("Прыжок", "Каждый слог подпрыгивает пружиной, когда его поют, и опускается с лёгким отскоком; спетое светится."),
                c!("Чётко", "Резкая подсветка буква за буквой точно по времени, без свечения и подъёма; остальные строки хорошо видны."),
            ],
            Self::Preset(Area::Track) => &[
                c!("Строго", "Квадратная обложка без кольца, ровное свечение фона."),
                c!("Живо", "Обложка-фигура вращается и дышит в такт, кольцо, свечение под звук."),
                c!("Максимум", "Случайные фигуры, кольцо, свечение под звук и фигуры на фоне."),
            ],
            Self::Preset(Area::Sources) => &[
                c!("Обычно", "Всё, кроме Soulseek, Squidify и SoundCloud."),
                c!("Всё", "Все источники."),
            ],
            Self::Preset(Area::Seeding) => &[
                c!("Не раздавать", "Скачанное сразу перестаёт раздаваться."),
                c!("Обычно", "До 50 ГБ, 30 дней, пока не отдашь в 3,5 раза больше скачанного."),
                c!("Щедро", "До 200 ГБ, 90 дней, без предела отдачи."),
            ],
            Self::Preset(Area::Load) => &[
                c!("Тихо", "60 кадров, окно в трее закрывается целиком, одна автозагрузка, без тяжёлой работы с голосом."),
                c!("Обычно", "120 кадров, быстрое открытие из трея, две автозагрузки, анализ стримов и отделение голоса."),
                c!("Быстро", "142 кадра, три автозагрузки и глубокое создание субтитров."),
            ],
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
                c!("Обычная", "Привычные цвета из обложки, как в Material You."),
                c!("Спокойная", "Мягкие, приглушённые цвета из обложки."),
                c!("Яркая", "Насыщенные, сочные цвета из обложки."),
                c!("Смелая", "Неожиданные сочетания: оттенки уходят в сторону от обложки."),
                c!("Точно как обложка", "Цвета точь-в-точь как на обложке, даже у кнопок."),
                c!("Чёрно-белая", "Только чёрный, белый и серый, без цвета."),
                c!("Сдержанная", "Почти серые цвета с лёгким оттенком обложки."),
            ],
            Self::NpVolume => &[
                c!("Ползунок", "Ползунок всегда рядом с кнопкой звука."),
                c!("Плашка", "Внизу по центру всплывает плашка, пока меняешь громкость, и прячется через пару секунд."),
                c!("Только кнопка", "Только кнопка звука; громкость — колесом мыши."),
            ],
            Self::CoverShape => &[
                c!("Квадрат", "Обложка как есть, обычный квадрат."),
                c!("Фигура", "Обложка вырезана одной фигурой из сетки."),
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
            Self::CoverDir => &[
                c!("По часовой", "Поворачивается как стрелки часов: 12 → 3 → 6."),
                c!("Против", "Поворачивается против часовой стрелки: 12 → 9 → 6."),
            ],
            Self::AlbumDate => &[
                c!("Год", "Только год: 2026."),
                c!("Месяц", "Месяц и год: октябрь 2026."),
                c!("Полностью", "День, месяц и год: 23 октября 2026."),
            ],
            Self::RingStyle => &[
                c!("Круг", "Точки стоят ровным кругом вокруг обложки."),
                c!("Контур", "Точки повторяют форму обложки."),
                c!("Бары", "Вокруг обложки столбики, которые растут под музыку, как эквалайзер."),
            ],
            Self::Ambient => &[
                c!("Выкл", "Обычный ровный фон."),
                c!("Ровное", "Цветные пятна медленно плывут за экраном."),
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
                c!("Фокус", "Строка, которую поют, выходит вперёд, а остальные отходят назад; ярко горит только слово или слог, который поют сейчас."),
                c!("Волна", "Каждый слог, буква или слово подпрыгивает, когда его поют: взлетает пружиной, чуть набухая, и опускается с лёгким отскоком; спетое остаётся чуть выше строки."),
            ],
            Self::LyricsUnit => &[
                c!("Буквы", "Караоке закрашивает слова буква за буквой, на протяжном слоге задерживаясь на гласной. В «Волне» прыгает каждая буква, «Фокус» зажигает строку целиком."),
                c!("Слоги", "Каждый слог заливается, пока его поют: быстрые слоги пробегают быстро, протяжный светится всю ноту. В «Волне» слово поднимается со своими слогами."),
                c!("Слова", "Караоке закрашивает каждое слово ровно, от первого звука до последнего; в «Волне» поднимается всё слово, в «Фокусе» горит поющееся слово."),
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
            Self::AlbumOpens => &[
                c!("Автоматически", "Альбом, добавленный в библиотеку, показывает и играет весь треклист. Не добавленный — только твои песни из него."),
                c!("Всегда весь альбом", "Любой альбом показывает и играет весь треклист, даже если твоя в нём одна песня. Песни, которые ты не выбирал, не считаются недостающими."),
                c!("Только мои песни", "Любой альбом, даже добавленный, показывает и играет только песни, которые у тебя есть, лайкнуты, в плейлистах или добавлены тобой."),
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
                c!("Изменения в обе стороны", "Правки здесь уходят в сервис, правки там приходят сюда."),
                c!("Только получать изменения", "Плейлист повторяет сервис; свои правки здесь туда не уходят."),
                c!("Скопировать один раз", "Плейлист скопирован один раз и дальше живёт сам по себе."),
            ],
            Self::RadioSource => &[
                c!("Скачанное", "Только треки с файлом: без стримов и без новой музыки. Это же включает «Только из скачанного»."),
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
            Self::RadioExplore => &[
                c!("Никогда", "Только новое, похожее по звуку на то, что играет."),
                c!("Редко", "Примерно одна песня из десяти."),
                c!("Иногда", "Примерно одна песня из пяти."),
                c!("Часто", "Примерно три песни из десяти, если в «Новом в радио и миксах» выбрано «Побольше»."),
            ],
            Self::SoundNeural => &[
                c!("Выключен", "Только базовый анализ: ничего не скачивается."),
                c!("Только моя библиотека", "Тихо слушает в фоне твои песни."),
                c!("Библиотека и превью новых песен", "Ещё и слушает превью новых песен, прежде чем они попадут в миксы. Рекомендуется."),
            ],
            Self::MixCardsWave => &[
                c!("Запускают Волну с этим вайбом", "Бесконечный поток в духе микса: подстраивается под пропуски и приносит новое. Рекомендуется."),
                c!("Открывают готовый список треков", "Как раньше: нажатие открывает страницу микса с песнями, ▶ играет его."),
                c!("Сразу играют микс", "Нажатие включает готовый список микса, его страница — в ⋮ → «Список треков»."),
            ],
            Self::SoundDevice => &[
                c!("Автоматически", "На процессоре, пока он свободен; на видеокарте, когда процессор занят, а у видеокарты есть свободная память."),
                c!("Процессор", "В один тихий поток; видеокарта не занята."),
                c!("Видеокарта", "Быстрее, но занимает около 300 МБ памяти."),
            ],
            Self::SearchPlay => &[
                c!("Радио по треку", "Трек играет один, дальше — похожие на него, как в автовоспроизведении."),
                c!("Остальные результаты поиска", "Следом играют остальные найденные треки по порядку."),
            ],
            Self::RadioUnheard => &[
                c!("Как обычно", "Слышанное и неслышанное — по тому, что подходит лучше."),
                c!("Больше новых", "Не меньше половины песен — те, что ты ещё не слышал."),
                c!("Только новые", "Только неслышанное, пока оно есть."),
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
            Self::FakeFlac => &[
                c!("Не проверять", "Файлы не проверяются: FLAC считается FLAC."),
                c!("Только помечать", "Файл остаётся как есть и подписан тем, что он на самом деле."),
                c!("Спрашивать", "Явный ненастоящий FLAC (MP3 внутри, резкий срез) ждёт в «Подтверди»: можно превратить его в MP3 320 (или V0, если исходник был слабее) — звук тот же, места меньше — или оставить. Теги, обложка и текст остаются, FLAC уходит в Корзину на 30 дней. Нужен ffmpeg."),
                c!("Менять на MP3 сам", "Явный ненастоящий FLAC сразу становится MP3, без вопроса. FLAC 30 дней лежит в Корзине."),
            ],
            Self::ListShuffle(_) => &[
                c!("Вперемешку", "Новый список играет вперемешку: перемешивание включается."),
                c!("По порядку", "Новый список играет по порядку: перемешивание выключается."),
                c!("По кнопке", "Как стоит кнопка перемешивания в плеере."),
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

    /// The options of a choice row in the active process locale.
    pub fn choices(self) -> &'static [Choice] {
        self.choices_in(nikgob_core::i18n::locale())
    }

    /// Which of [`Self::choices`] `p` has.
    pub fn chosen(self, p: &Prefs) -> usize {
        match self {
            // None matching: «Свои», the button after the presets.
            Self::Preset(a) => a.current(p).unwrap_or(a.names().len()),
            Self::Language => nikgob_core::i18n::LangPref::ALL.iter().position(|l| *l == p.language).unwrap_or(0),
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
            Self::AlbumDate => match p.album_date {
                v::AlbumDate::Year => 0,
                v::AlbumDate::MonthYear => 1,
                v::AlbumDate::Full => 2,
            },
            Self::LyricsGpu => usize::from(!p.lyrics_gpu),
            Self::LyricsStreamKeep => usize::from(p.lyrics_stream_keep.min(1)),
            Self::LyricsUi => usize::from(p.lyrics_ui.min(2)),
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
            Self::FakeFlac => match (p.check_fake_lossless, p.convert_fake_lossless, p.fake_flac_auto) {
                (false, _, _) => 0,
                (true, false, _) => 1,
                (true, true, false) => 2,
                (true, true, true) => 3,
            },
            Self::LikeDownload if !p.like_downloads => 2,
            Self::LikeDownload => usize::from(p.like_download_scope == v::WishlistScope::Album),
            Self::UpgradeEvery => v::UpgradeEvery::ALL.iter().position(|e| *e == p.upgrade_every).unwrap_or(1),
            Self::UpgradeBar => usize::from(p.upgrade_below == v::UpgradeBar::Mp3_320),
            Self::UpgradeAuto => usize::from(p.upgrade_auto),
            Self::PlaylistSync => sync_index(p.playlist_sync_default),
            Self::AlbumLike => usize::from(p.album_like_tracks),
            Self::AlbumOpens => match p.album_mode {
                v::AlbumMode::Auto => 0,
                v::AlbumMode::Whole => 1,
                v::AlbumMode::Mine => 2,
            },
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
            Self::SearchPlay => usize::from(p.search_play == v::SearchPlay::Results),
            Self::RadioExplore => v::RadioExplore::ALL.iter().position(|e| *e == p.radio_explore).unwrap_or(1),
            Self::RadioUnheard => v::RadioUnheard::ALL.iter().position(|u| *u == p.radio_unheard).unwrap_or(0),
            Self::SoundNeural => v::SoundNeural::ALL.iter().position(|m| *m == p.sound_neural_or(false)).unwrap_or(0),
            Self::SoundDevice => v::SoundDevice::ALL.iter().position(|d| *d == p.sound_device).unwrap_or(0),
            Self::MixCardsWave => super::cards::MixClick::ALL.iter().position(|c| *c == super::cards::MixClick::chosen(p)).unwrap_or(0),
            Self::ListShuffle(kind) => v::ListShuffle::ALL.iter().position(|r| *r == p.list_shuffle.of(kind)).unwrap_or(2),
            _ => 0,
        }
    }

    /// Sets option `i` of [`Self::choices`] in `p`.
    pub fn choose(self, p: &mut Prefs, i: usize) {
        match self {
            // As `PrefPatch::Preset` does it (from «Свои», the values are kept).
            Self::Preset(a) => a.apply(p, i),
            Self::Language => {
                p.language = nikgob_core::i18n::LangPref::ALL[i.min(nikgob_core::i18n::LangPref::ALL.len() - 1)];
            }
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
            Self::AlbumDate => p.album_date = [v::AlbumDate::Year, v::AlbumDate::MonthYear, v::AlbumDate::Full][i.min(2)],
            Self::LyricsGpu => p.lyrics_gpu = i == 0,
            Self::LyricsStreamKeep => p.lyrics_stream_keep = i.min(1) as u8,
            Self::LyricsUi => p.lyrics_ui = i.min(2) as u8,
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
            // Not checking leaves the other two as they were (they come
            // back with the check).
            Self::FakeFlac => match i {
                0 => p.check_fake_lossless = false,
                1 => (p.check_fake_lossless, p.convert_fake_lossless) = (true, false),
                2 => (p.check_fake_lossless, p.convert_fake_lossless, p.fake_flac_auto) = (true, true, false),
                _ => (p.check_fake_lossless, p.convert_fake_lossless, p.fake_flac_auto) = (true, true, true),
            },
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
            Self::AlbumOpens => {
                p.album_mode = match i {
                    1 => v::AlbumMode::Whole,
                    2 => v::AlbumMode::Mine,
                    _ => v::AlbumMode::Auto,
                }
            }
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
            Self::SearchPlay => p.search_play = if i == 1 { v::SearchPlay::Results } else { v::SearchPlay::Radio },
            Self::RadioExplore => p.radio_explore = v::RadioExplore::ALL[i.min(v::RadioExplore::ALL.len() - 1)],
            Self::RadioUnheard => p.radio_unheard = v::RadioUnheard::ALL[i.min(v::RadioUnheard::ALL.len() - 1)],
            Self::SoundNeural => p.sound_neural = Some(v::SoundNeural::ALL[i.min(v::SoundNeural::ALL.len() - 1)]),
            Self::SoundDevice => p.sound_device = v::SoundDevice::ALL[i.min(v::SoundDevice::ALL.len() - 1)],
            Self::MixCardsWave => super::cards::MixClick::ALL[i.min(2)].choose(p),
            Self::ListShuffle(kind) => p.list_shuffle.set(kind, v::ListShuffle::ALL[i.min(v::ListShuffle::ALL.len() - 1)]),
            _ => {}
        }
    }

    fn preview(self) -> Option<Preview> {
        use SettingId::*;
        Some(match self {
            Theme | Contrast | Scheme => Preview::Palette,
            Ambient | AmbientStrength => Preview::Ambient,
            ProgressWave | ReducedMotion => Preview::Wave,
            CoverDir | CoverSpin | CoverSpinSpeed => Preview::Turn,
            CoverShape | ShapeRange | ShapePick | CoverPulse | CoverPulseStrength => Preview::Cover,
            Ring | RingStyle => Preview::Ring,
            LyricsCascade => Preview::Cascade,
            BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue => Preview::Background,
            LyricsStyle | WaveFallback | WaveJump | JumpSpeed | LightSpeed | LyricsSmooth | LyricsDim | LyricsScale | LyricsGlow | LyricsUnit
            | LyricsLift => Preview::Lyrics,
            LyricsSoftEdges | LyricsSoftStrength | LyricsSoftDepth | LyricsSoftFade => Preview::Edges,
            LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize => Preview::Gap,
            LyricsCountdown => Preview::Countdown,
            NpMotion => Preview::Motion,
            NpVolume | NpVolumeHold => Preview::Volume,
            _ => return None,
        })
    }
}

/// The title of the group an area's presets head.
pub fn preset_title(a: v::presets::Area) -> &'static str {
    use v::presets::Area;
    match a {
        Area::Recs => "Рекомендации",
        Area::Lyrics => "Вид текста",
        Area::Track => "Экран трека",
        Area::Sources => "Откуда качать",
        Area::Seeding => "Раздача",
        Area::Load => "Нагрузка",
    }
}

/// What the live stage shows for a row.
impl SettingId {
    /// The screen on the stage while the pointer is on the row; `None`: the
    /// one there already (the theme shows on any).
    pub fn scene(self) -> Option<Scene> {
        use SettingId::*;
        match self {
            NpMotion | Preset(v::presets::Area::Track) | PlayerButton(_) => Some(Scene::Track),
            Preset(v::presets::Area::Lyrics) => Some(Scene::Lyrics),
            HomeSections => Some(Scene::Home),
            LikedAlbumsPlace => Some(Scene::Album),
            _ => self.part().and_then(Part::scene),
        }
    }

    /// The part of the screen the row changes; `None`: the row is not on
    /// the stage. [`Part::Whole`]: all of it, nothing outlined.
    pub fn part(self) -> Option<Part> {
        use SettingId::*;
        Some(match self {
            Theme | Contrast | Scheme | ReducedMotion | NpMotion | HomeSections | PlayerButton(_) => Part::Whole,
            Preset(v::presets::Area::Track | v::presets::Area::Lyrics) => Part::Whole,
            CoverShape | ShapeRange | ShapePick | CoverSpin | CoverDir | CoverSpinSpeed | CoverPulse | CoverPulseStrength
            | CoverPauseMorph | CoverClickPause | DblClickShape | TapWait => Part::Cover,
            Ring | RingStyle => Part::Ring,
            Ambient | AmbientStrength | BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue => {
                Part::Background
            }
            ProgressWave => Part::Progress,
            NpVolume | NpVolumeHold => Part::Volume,
            LyricsStyle | LyricsUnit | LyricsSmooth | LightSpeed | WaveJump | JumpSpeed | LyricsLift | LyricsLead | LyricsDim
            | LyricsGlow | WaveFallback => Part::LyricsLine,
            LyricsScale | LyricsCascade | LyricsReturn => Part::LyricsColumn,
            LyricsSoftEdges | LyricsSoftDepth | LyricsSoftStrength | LyricsSoftFade => Part::LyricsEdges,
            LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize | LyricsCountdown => Part::LyricsPause,
            MixesProfiles | MixesEverything | MixesMoods | MixesOnRepeat | BackgroundCards | ProfileNames
            | HiddenMixes => Part::Home(Section::Mixes),
            AlbumDate => Part::AlbumDate,
            _ => return None,
        })
    }

    /// The parts outlined for the row: its own, or every part a preset
    /// changes (none for [`Part::Whole`]).
    pub fn parts(self) -> Vec<Part> {
        use v::presets::Area;
        match self {
            Self::Preset(Area::Track) => vec![Part::Cover, Part::Ring, Part::Background, Part::Progress],
            Self::Preset(Area::Lyrics) => vec![Part::LyricsLine, Part::LyricsColumn],
            _ => self.part().filter(|p| *p != Part::Whole).into_iter().collect(),
        }
    }

    /// The settings the stage shows for the row from `p`: where the row
    /// tunes something the shown look does not have (a jump without the
    /// wave, crests on a flat ring, a speed of a line that lights whole),
    /// the look that has it, so every knob shows on the stage. Nothing of it
    /// is kept.
    pub fn staged(self, p: &Prefs) -> Prefs {
        use SettingId::*;
        let mut q = p.clone();
        match self {
            WaveJump | JumpSpeed if q.lyrics_style != 2 => q.lyrics_style = 2,
            LyricsLift if q.lyrics_style != 0 => q.lyrics_style = 0,
            LyricsGapWaves | LyricsGapHeight if q.lyrics_gap_style % 2 == 0 => q.lyrics_gap_style += 1,
            LyricsSmooth | LightSpeed if q.lyrics_style == 1 && q.lyrics_unit == v::LYRICS_BY_LETTERS => {
                q.lyrics_unit = v::LYRICS_BY_SYLLABLES;
            }
            _ => {}
        }
        q
    }

    /// What the stage acts out for the row with the settings `p` shown: what
    /// cannot be seen without doing something.
    pub fn script(self, p: &Prefs) -> Script {
        use SettingId::*;
        match self {
            // A shape of the pool under the pointer is shown as it is.
            CoverShape | ShapeRange if p.cover_shape == v::CoverShape::Random => Script::RandomShape,
            CoverPauseMorph | CoverClickPause => Script::PausePlay,
            DblClickShape | TapWait => Script::DoubleTap,
            NpVolume | NpVolumeHold => Script::Volume,
            NpMotion => Script::Open,
            LyricsCascade => Script::LyricsStep,
            LyricsReturn => Script::LyricsReturn,
            LyricsSoftEdges | LyricsSoftDepth | LyricsSoftStrength | LyricsSoftFade => Script::EdgesScroll,
            LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize => Script::Interlude,
            LyricsCountdown => Script::Countdown,
            WaveFallback => Script::LineTimes,
            _ => Script::None,
        }
    }

    /// Whether the stage should pulse on a beat while this row is shown.
    /// Cover shape, ring style and the rest stay calm, so the shape can be
    /// judged. Only a row whose change is the beat itself pulses, and only
    /// while that switch is on: pulse off, or ring beat off, stays calm.
    pub fn needs_beat(self, p: &Prefs) -> bool {
        use SettingId::*;
        match self {
            Ambient | AmbientStrength => p.ambient == v::Ambient::Reactive,
            Ring => p.ring,
            CoverPulse | CoverPulseStrength => p.cover_pulse,
            BgReact => p.bg_shapes != v::BgShapes::Off,
            Preset(v::presets::Area::Track) => {
                p.cover_pulse
                    || p.ring
                    || p.ambient == v::Ambient::Reactive
                    || (p.bg_shapes != v::BgShapes::Off && p.bg_shapes_react > 0.02)
            }
            _ => false,
        }
    }
}

/// The rows that show a field a preset owns.
pub fn preset_rows(f: v::presets::Field) -> Vec<SettingId> {
    use SettingId as S;
    use v::presets::Field as F;
    match f {
        F::CoverShape => vec![S::CoverShape],
        F::CoverSpin => vec![S::CoverSpin],
        F::CoverPulse => vec![S::CoverPulse],
        F::Ring => vec![S::Ring],
        // The ring's old rows: no row shows them now.
        F::RingBeat | F::SpinRing => Vec::new(),
        F::Ambient => vec![S::Ambient],
        F::BgShapes => vec![S::BgShapes],
        F::ProgressWave => vec![S::ProgressWave],
        F::MixesDownloadedOnly => vec![S::MixesDownloadedOnly],
        F::RadioNewShare => vec![S::RadioNewShare],
        F::RadioNewShareBy => [v::RadioKind::Track, v::RadioKind::Artist, v::RadioKind::Mix, v::RadioKind::Autoplay]
            .into_iter()
            .map(S::NewShareBy)
            .collect(),
        F::MixesWithNew => vec![S::MixesWithNew],
        F::RadioExplore => vec![S::RadioExplore],
        F::RadioUnheard => vec![S::RadioUnheard],
        F::Source(k) => vec![S::DownloadSource(k)],
        F::NoSeed => vec![S::NoSeed],
        F::SeedLimitGb => vec![S::SeedLimit],
        F::SeedDays => vec![S::SeedDays],
        F::SeedRatio => vec![S::SeedRatio],
        F::FpsLimit => vec![S::FpsLimit],
        F::TrayKeepWindow => vec![S::TrayKeepWindow],
        F::AutoDownloadLimit => vec![S::AutoDownloads],
        F::StreamAnalysis => vec![S::StreamAnalysis],
        F::LyricsSeparate => vec![S::LyricsSeparate],
        F::LyricsDeep => vec![S::LyricsDeep],
        F::LyricsStyle => vec![S::LyricsStyle],
        F::LyricsUnit => vec![S::LyricsUnit],
        F::LyricsLightSpeed => vec![S::LightSpeed],
        F::LyricsSmooth => vec![S::LyricsSmooth],
        F::LyricsWaveJump => vec![S::WaveJump],
        F::LyricsJumpSpeed => vec![S::JumpSpeed],
        F::LyricsLift => vec![S::LyricsLift],
        F::LyricsDim => vec![S::LyricsDim],
        F::LyricsGlow => vec![S::LyricsGlow],
        F::LyricsCascade => vec![S::LyricsCascade],
        F::LyricsSoftEdges => vec![S::LyricsSoftEdges],
    }
}

/// A field's name in a preset's card (shorter than its row's, and never
/// the same for two fields of one area).
fn field_label(f: v::presets::Field) -> &'static str {
    use v::presets::Field as F;
    match f {
        F::CoverShape => "Форма обложки",
        F::CoverSpin => "Вращение обложки",
        F::CoverPulse => "Пульсация",
        F::Ring => "Кольцо",
        F::RingBeat => "Кольцо в такт",
        F::SpinRing => "Вращение кольца",
        F::Ambient => "Свечение фона",
        F::BgShapes => "Фигуры на фоне",
        F::ProgressWave => "Волна на полосе",
        F::MixesDownloadedOnly => "Только скачанное",
        F::RadioNewShare => "Новое",
        F::RadioNewShareBy => "Новое по видам",
        F::MixesWithNew => "С новым в миксах",
        F::RadioExplore => "Случайные песни",
        F::RadioUnheard => "Неслышанные",
        F::Source(k) => k.display_name(),
        F::NoSeed => "Не раздавать",
        F::SeedLimitGb => "Лимит",
        F::SeedDays => "Срок",
        F::SeedRatio => "Предел отдачи",
        F::FpsLimit => "Кадры",
        F::TrayKeepWindow => "Быстро из трея",
        F::AutoDownloadLimit => "Автозагрузки",
        F::StreamAnalysis => "Анализ стримов",
        F::LyricsSeparate => "Отделять голос",
        F::LyricsDeep => "Глубокие субтитры",
        F::LyricsStyle => "Анимация",
        F::LyricsUnit => "Шаг",
        F::LyricsLightSpeed => "Скорость подсветки",
        F::LyricsSmooth => "Плавность",
        F::LyricsWaveJump => "Высота прыжка",
        F::LyricsJumpSpeed => "Скорость прыжка",
        F::LyricsLift => "Подъём спетого",
        F::LyricsDim => "Приглушение",
        F::LyricsGlow => "Свечение",
        F::LyricsCascade => "Каскад",
        F::LyricsSoftEdges => "Мягкие края",
    }
}

/// A value a preset writes, as its card shows it.
fn field_text(f: v::presets::Field, value: &v::presets::FieldValue) -> String {
    use v::presets::FieldValue as V;
    // A choice: the label its row shows for it.
    let choice = || {
        let rows = preset_rows(f);
        let id = rows.first().copied()?;
        let mut p = Prefs::default();
        f.set(&mut p, value);
        id.choices().get(id.chosen(&p)).map(|c| c.label.to_owned())
    };
    match value {
        V::Bool(true) => "да".to_owned(),
        V::Bool(false) => "нет".to_owned(),
        V::U32(n) => match f {
            // 0 is no cap (the torrents' policy), not «0 ГБ».
            v::presets::Field::SeedLimitGb if *n == 0 => "без лимита".to_owned(),
            v::presets::Field::SeedLimitGb => format!("{n} ГБ"),
            v::presets::Field::SeedDays => format!("{n} дн."),
            _ => n.to_string(),
        },
        // The look of the lyrics: shares, in percent.
        V::F32(x)
            if matches!(
                f,
                v::presets::Field::LyricsLightSpeed
                    | v::presets::Field::LyricsSmooth
                    | v::presets::Field::LyricsWaveJump
                    | v::presets::Field::LyricsJumpSpeed
                    | v::presets::Field::LyricsLift
                    | v::presets::Field::LyricsDim
            ) =>
        {
            format!("{} %", (x * 100.0).round())
        }
        V::F32(x) if x.abs() < 1e-3 => "без предела".to_owned(),
        V::F32(x) => format!("{x:.1}").replace('.', ","),
        V::Shape(v::CoverShape::Original) => "Квадрат".to_owned(),
        V::Shape(v::CoverShape::Random) => "Случайная".to_owned(),
        V::Shape(c) => c.label().to_owned(),
        V::ShareBy(_) => "—".to_owned(),
        _ => choice().unwrap_or_else(|| "—".to_owned()),
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
    /// The widget with the keyboard focus is in it.
    focused: bool,
    /// How it was drawn: faded by a row it depends on, or switched off.
    opacity: f32,
    enabled: bool,
    /// The title of the group it is in ([`note_group`]).
    group: String,
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
    /// A preset's: the explanation, then its fields × its presets, the
    /// current preset's column marked.
    Table,
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

fn group_key() -> Id {
    Id::new("settings-help-group")
}

fn pinned_key() -> Id {
    Id::new("settings-help-pinned")
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
        focused: ctx
            .memory(|m| m.focused())
            .and_then(|f| ctx.read_response(f))
            .is_some_and(|r| rect.union(button).contains(r.rect.center())),
        opacity: ui.opacity(),
        enabled: ui.is_enabled(),
        group: ctx.data(|d| d.get_temp::<String>(group_key())).unwrap_or_default(),
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

/// The rect of the item's control (the sweep checks it does not cover the ⓘ).
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

/// After the page: scrolls to a linked row and shows the pinned card. A
/// pick in the card lands in `prefs`. Beside the live stage (`samples`
/// false) the card has no sample of its own: the stage shows it.
pub fn finish(ui: &mut Ui, prefs: &mut Prefs, samples: bool) {
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
        // A row that never shows (another level, a hidden list) lets go too.
        let gone = anim::now(&ctx) - f.start > lasts + 1.0;
        if (f.scrolls == 0 && anim::now(&ctx) - f.start > lasts) || gone {
            ctx.data_mut(|d| d.remove::<Focus>(focus_key()));
        } else {
            ctx.data_mut(|d| d.insert_temp(focus_key(), f));
        }
    }

    let pass = ctx.cumulative_pass_nr();
    let rows = ctx.data(|d| d.get_temp::<Rows>(rows_key())).filter(|r| r.pass == pass).map(|r| r.list).unwrap_or_default();

    if let Some(mut pin) = pinned(&ctx) {
        // Gone with its ⓘ (another tab, hidden, scrolled away): the card
        // opens next to that button, so it has nothing to hold on to.
        let Some(row) = rows.iter().find(|r| r.id == pin.id && r.clip.intersects(r.button)) else {
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
        // The row's current option, or the option whose button (or item
        // of its open menu) the pointer is on.
        let now = anim::now(&ctx);
        let pointer = ctx.pointer_hover_pos();
        let n = row.id.choices().len();
        let on = |p: Pos2| row.choices.iter().position(|r| r.contains(p)).or_else(|| row.menu.iter().position(|r| r.contains(p)));
        let body = match pointer.and_then(on).filter(|i| *i < n) {
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
        let loc = prefs.resolved_locale();
        let layout = Layout::new(&ctx, row, body, samples, loc);
        // Next to this row's own ⓘ, not under the row: a group title's row
        // is the whole list, and anchoring there dropped the card in the corner.
        let anchor = row.button;
        let target = Rect::from_min_size(place(anchor, layout.size, ctx.content_rect()), layout.size);
        // A short fade and rise from the row (effects timing, no bounce).
        let t = ((anim::now(&ctx) - pin.at) / 0.18).clamp(0.0, 1.0) as f32;
        let k = 1.0 - (1.0 - t).powi(3);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let rise = if target.top() >= anchor.bottom() { -1.0 } else { 1.0 } * 8.0 * (1.0 - k);
        let rect = target.translate(vec2(0.0, -rise));
        let out = show_card(&ctx, Id::new("settings-help-pinned-card"), rect, k, content, &layout, row, prefs, true);
        if let Some(i) = out.pick {
            row.id.choose(prefs, i);
        }
        pin.card = Some(rect);
        if !out.close {
            // A press outside the row and the card puts the card away and
            // reaches no slider under it.
            crate::components::slider::note_dismissible(&ctx, row.rect);
            crate::components::slider::note_dismissible(&ctx, rect);
        }
        set_pinned(&ctx, (!out.close).then_some(pin));
    }
}

/// What the ⓘ card of `id` shows: the explanation and the current
/// option's preview, or the options as lines when nothing is previewed.
fn base_body(id: SettingId) -> Body {
    if matches!(id, SettingId::Preset(_)) {
        Body::Table
    } else if !id.choices().is_empty() && id.preview().is_none() {
        Body::All
    } else {
        Body::Row
    }
}

/// Text width of a one-thing card.
const CARD_W: f32 = 280.0;
/// Text width of a preset's card (its table).
const TABLE_W: f32 = 360.0;
/// A line of a preset's table.
const TABLE_ROW: f32 = 20.0;
const PREVIEW_H: f32 = 120.0;
const PAD: f32 = 12.0;
/// Between a row's ⓘ and its card.
const GAP: f32 = 6.0;

/// A paragraph in `role`, wrapped at `width`.
fn para(painter: &Painter, role: TypeRole, text: &str, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), role.style().text_format(color));
    job.wrap.max_width = width;
    painter.layout_job(job)
}

/// Where a card of `size` opens next to `button`: just under it, from the
/// button's left, or just above when there is no room below; grown left
/// from the button's right edge when it would leave the screen on the right.
/// On screen, and over the button only when it fits neither way.
fn place(button: Rect, size: Vec2, screen: Rect) -> Pos2 {
    // Keep off the window's edges: more at the sides than at the bottom,
    // where a few px decide whether the card fits under its button.
    let screen = screen.shrink2(vec2(8.0, 4.0));
    let below = button.bottom() + GAP;
    let above = button.top() - GAP - size.y;
    let y = if below + size.y <= screen.bottom() {
        below
    } else if above >= screen.top() {
        above
    } else if screen.bottom() - button.bottom() >= button.top() - screen.top() {
        (screen.bottom() - size.y).max(screen.top())
    } else {
        screen.top()
    };
    // Next to the ⓘ: grow right from its left edge. If that leaves the
    // screen, grow left from its right edge. If neither fits, stay on screen.
    let grow_right = button.left();
    let grow_left = button.right() - size.x;
    let fits = |x: f32| x >= screen.left() && x + size.x <= screen.right();
    let x = if fits(grow_right) {
        grow_right
    } else if fits(grow_left) {
        grow_left
    } else {
        grow_right.clamp(screen.left(), (screen.right() - size.x).max(screen.left()))
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

/// A card laid out: its size and its texts.
struct Layout {
    size: Vec2,
    body: Body,
    kind: Option<Preview>,
    title: Arc<Galley>,
    help: Arc<Galley>,
    /// [`Body::All`]: each option's name and what it does.
    texts: Vec<(Arc<Galley>, Arc<Galley>)>,
    /// [`Body::Table`].
    table: Option<Table>,
    /// The text's width.
    width: f32,
    /// A close button in the header (the pinned card).
    close: bool,
}

/// A preset's table: the presets across, the fields down (texts without a
/// colour of their own, painted in the colour of their column).
struct Table {
    heads: Vec<Arc<Galley>>,
    rows: Vec<(Arc<Galley>, Vec<Arc<Galley>>)>,
    label_w: f32,
}

impl Table {
    fn new(painter: &Painter, area: v::presets::Area, width: f32) -> Self {
        let text = |role: TypeRole, t: &str| para(painter, role, t, Color32::PLACEHOLDER, width);
        let heads: Vec<Arc<Galley>> = area.names().iter().map(|n| text(TypeRole::LabelMedium, *n)).collect();
        let n = heads.len();
        let rows: Vec<(Arc<Galley>, Vec<Arc<Galley>>)> = area
            .fields()
            .iter()
            .filter(|f| **f != v::presets::Field::RadioNewShareBy)
            .map(|&f| {
                let values = (0..n)
                    .map(|i| {
                        let t = area.values(i).iter().find(|(g, _)| *g == f).map_or_else(|| "—".to_owned(), |(_, v)| field_text(f, v));
                        text(TypeRole::BodySmall, &t)
                    })
                    .collect();
                (text(TypeRole::BodySmall, field_label(f)), values)
            })
            .collect();
        let label_w = rows.iter().map(|(l, _)| l.size().x).fold(0.0, f32::max).min(width * 0.42) + 8.0;
        Self { heads, rows, label_w }
    }

    fn height(&self) -> f32 {
        TABLE_ROW * (self.rows.len() + 1) as f32
    }

    /// Paints it from `(x, y)`, `width` wide, the column of preset
    /// `current` marked.
    fn paint(&self, painter: &Painter, x: f32, y: f32, width: f32, current: Option<usize>, s: &ColorScheme) {
        let n = self.heads.len().max(1);
        let col = (width - self.label_w) / n as f32;
        let at = |i: usize| x + self.label_w + i as f32 * col;
        if let Some(c) = current.filter(|c| *c < n) {
            let r = Rect::from_min_size(pos2(at(c) - 4.0, y - 2.0), vec2(col, self.height() + 4.0));
            painter.rect_filled(r, shape::SMALL, s.secondary_container);
        }
        let mid = |g: &Arc<Galley>, top: f32| top + (TABLE_ROW - g.size().y) * 0.5;
        for (i, h) in self.heads.iter().enumerate() {
            let color = if current == Some(i) { s.on_secondary_container } else { s.on_surface_variant };
            painter.galley(pos2(at(i), mid(h, y)), h.clone(), color);
        }
        for (k, (label, values)) in self.rows.iter().enumerate() {
            let top = y + TABLE_ROW * (k + 1) as f32;
            painter.galley(pos2(x, mid(label, top)), label.clone(), s.on_surface);
            for (i, v) in values.iter().enumerate() {
                let color = if current == Some(i) { s.on_secondary_container } else { s.on_surface_variant };
                painter.galley(pos2(at(i), mid(v, top)), v.clone(), color);
            }
        }
    }
}

/// The first words of a [`SettingId::beta`] row's card.
const BETA_CARD: &str = "Функция в бета-тестировании. ";

/// Room for the close button beside the title.
const CLOSE_W: f32 = 32.0;

impl Layout {
    fn new(ctx: &egui::Context, row: &Row, body: Body, samples: bool, loc: nikgob_core::i18n::Locale) -> Self {
        let pinned = true;
        let s = Theme::get(ctx).scheme;
        let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("settings-help-measure")));
        let choices = row.id.choices_in(loc);
        let kind = if matches!(body, Body::All | Body::Table) || !samples { None } else { row.id.preview() };
        let width = if body == Body::Table { TABLE_W } else { CARD_W };
        let (title, help) = match body {
            Body::Option(i) => (format!("{} · {}", row.label, choices[i].label), choices[i].help.to_owned()),
            _ if row.id.beta() => (row.label.clone(), BETA_CARD.to_owned() + row.id.help_in(loc)),
            _ => (row.label.clone(), row.id.help_in(loc).to_owned()),
        };
        let close_w = if pinned { CLOSE_W } else { 0.0 };
        let title = para(&painter, TypeRole::TitleSmall, &title, s.on_surface, width - close_w);
        let help = para(&painter, TypeRole::BodyMedium, &help, s.on_surface_variant, width);
        let table = match (body, row.id) {
            (Body::Table, SettingId::Preset(area)) => Some(Table::new(&painter, area, width)),
            _ => None,
        };
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
        if let Some(t) = &table {
            h += 10.0 + t.height();
        }
        h += PAD;
        Self { size: vec2(width + 2.0 * PAD, h), body, kind, title, help, texts, table, width, close: pinned }
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
            if let (Some(t), SettingId::Preset(area)) = (&layout.table, row.id) {
                y += 10.0;
                t.paint(ui.painter(), x0, y, layout.width, area.current(prefs), &s);
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
        Preview::Turn => turn_preview(ui, rect, prefs),
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

/// Which way the cover's outline turns: a clock face, the
/// shape turning in it with a bright marker, and an arrow running ahead of
/// the marker the same way. Clockwise goes 12 → 3 → 6.
fn turn_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    use std::f32::consts::TAU;
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let ccw = prefs.cover_ccw;
    let sign = if ccw { -1.0 } else { 1.0 };
    // Not turning at all: no marker run, no arrow.
    let turning = prefs.cover_spin;
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
    let shape = match prefs.cover_shape {
        v::CoverShape::Original | v::CoverShape::Random => v::CoverShape::Cookie12,
        other => other,
    };
    if let Some(m) = now_playing::material(shape) {
        let box_ = Rect::from_center_size(c, Vec2::splat((r - 6.0) * 2.0));
        nikgob_shapes::paint::fill_shape(&painter, now_playing::shape_cubics(m), box_, a.rem_euclid(TAU), s.secondary_container);
    }
    // The marker on the turning part, and an arrow ahead of it.
    painter.circle_filled(c + dir(a) * (r - 12.0), 4.5, s.primary);
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

/// The settings group being drawn is titled `title` (the rows after it
/// carry it, for the settings sweep).
pub fn note_group(ctx: &egui::Context, title: &str) {
    ctx.data_mut(|d| d.insert_temp(group_key(), title.to_owned()));
}

/// One settings row as the page drew it (the settings sweep reads them).
#[derive(Clone, Debug)]
pub struct DrawnRow {
    pub id: SettingId,
    pub label: String,
    pub group: String,
    /// The row with its ⓘ, its ⓘ, its control and the control's options.
    pub rect: Rect,
    pub button: Rect,
    pub control: Option<Rect>,
    pub choices: Vec<Rect>,
    /// The items of its open menu, in [`SettingId::choices`] order.
    pub menu: Vec<Rect>,
    /// What of the page is on screen around it.
    pub clip: Rect,
    pub opacity: f32,
    pub enabled: bool,
    /// The pointer is on it (or its ⓘ).
    pub hovered: bool,
    /// The widget with the keyboard focus is in it.
    pub focused: bool,
}

/// The rows of the last pass that drew any, with that pass's number.
pub fn drawn_rows(ctx: &egui::Context) -> (u64, Vec<DrawnRow>) {
    let rows = ctx.data(|d| d.get_temp::<Rows>(rows_key())).unwrap_or_default();
    let list = rows
        .list
        .into_iter()
        .map(|r| DrawnRow {
            id: r.id,
            label: r.label,
            group: r.group,
            rect: r.rect,
            button: r.button,
            control: r.control,
            choices: r.choices,
            menu: r.menu,
            clip: r.clip,
            opacity: r.opacity,
            enabled: r.enabled,
            hovered: r.hovered,
            focused: r.focused,
        })
        .collect();
    (rows.pass, list)
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

    /// Every row knows its tab, level, name and ⓘ text.
    #[test]
    fn every_row_has_a_tab_a_level_a_name_and_help() {
        for &id in crate::screens::sweep::ALL {
            assert!(id.tab() < TABS.len(), "{id:?}");
            assert!(!id.label().is_empty(), "{id:?}");
            assert!(!id.help().is_empty(), "{id:?}");
            // A name says the thing, never its value.
            assert!(!id.label().contains(':') && !id.label().ends_with(" с"), "{id:?}: {}", id.label());
        }
    }

    /// «Просто» has whether a thing is there and which kind of it;
    /// «Подробно» adds how strong, how fast and how long, the rare cases
    /// and the technical rows.
    #[test]
    fn simple_has_the_thing_and_deep_its_tuning() {
        use SettingId as S;
        for id in [
            // The animation, its step, each effect's switch.
            S::LyricsStyle, S::LyricsUnit, S::LyricsScale, S::LyricsLead, S::LyricsCascade, S::LyricsGlow, S::LyricsSoftEdges,
            S::LyricsGap, S::LyricsCountdown, S::LyricsWordsPlaying, S::LyricsLinesAuto, S::LyricsStreams,
            // Each part of the track screen.
            S::Contrast, S::CoverShape, S::CoverSpin, S::CoverPulse, S::Ring, S::Ambient, S::BgShapes,
            S::ProgressWave, S::NpVolume, S::HiddenMixes,
            S::SpotifyPlayback, S::ImportSpotify, S::DownloadToasts, S::Inbox, S::Trash,
        ] {
            assert_eq!(id.level(), Level::Simple, "{id:?}");
        }
        for id in [
            S::LyricsSmooth, S::LightSpeed, S::WaveJump, S::JumpSpeed, S::LyricsLift, S::LyricsDim, S::LyricsSoftDepth,
            S::LyricsSoftStrength, S::LyricsSoftFade, S::LyricsGapWaves, S::LyricsGapHeight, S::LyricsGapSize, S::LyricsReturn, S::WaveFallback,
            S::LyricsModels, S::CoverDir, S::CoverSpinSpeed, S::CoverPulseStrength, S::RingStyle,
            S::AmbientStrength, S::BgCount, S::NpVolumeHold, S::TapWait, S::ToastTime, S::StreamCache,
            S::SeedRatio, S::FpsLimit,
        ] {
            assert_eq!(id.level(), Level::Deep, "{id:?}");
        }
        // A row that hangs on another shows no earlier than it does.
        for (child, parent) in [
            (S::RingStyle, S::Ring),
            (S::AutostartHidden, S::Autostart),
            (S::LyricsSoftDepth, S::LyricsSoftEdges),
            (S::LyricsGapSize, S::LyricsGap),
            (S::LyricsStreamKeep, S::LyricsStreams),
            (S::KeepTimbre, S::TrackSpeed),
            (S::AutoplayWorld, S::Autoplay),
            (S::CoverSpinSpeed, S::CoverSpin),
            (S::CoverPulseStrength, S::CoverPulse),
            (S::AmbientStrength, S::Ambient),
            (S::BgCount, S::BgShapes),
            (S::NpVolumeHold, S::NpVolume),
        ] {
            assert!(child.level() >= parent.level(), "{child:?} under {parent:?}");
        }
    }

    /// Links from other screens land on the tab that has the row now.
    #[test]
    fn linked_rows_know_their_new_tab() {
        use SettingId as S;
        for (id, tab) in [
            (S::LyricsStyle, TAB_LYRICS),
            (S::LyricsUi, TAB_SYSTEM),
            (S::LyricsModels, TAB_LYRICS),
            (S::BgHue, TAB_LOOK),
            (S::MixesProfiles, TAB_LOOK),
            (S::HiddenMixes, TAB_LOOK),
            (S::LikedAlbumsPlace, TAB_LOOK),
            (S::CoverClickPause, TAB_LOOK),
            (S::Prowlarr, TAB_DOWNLOADS),
            (S::NoSeed, TAB_DOWNLOADS),
            (S::SeedRatio, TAB_DOWNLOADS),
            (S::UploadLimit, TAB_DOWNLOADS),
            (S::Soulseek, TAB_DOWNLOADS),
            (S::SoulseekUpload, TAB_DOWNLOADS),
            (S::DownloadSource(v::SourceKey::Torrents), TAB_DOWNLOADS),
            (S::LikeDownload, TAB_DOWNLOADS),
            (S::Spotify, TAB_SERVICES),
            (S::SpotifyPlayback, TAB_SERVICES),
            (S::YouTube, TAB_SERVICES),
            (S::PlaylistSync, TAB_SERVICES),
            (S::StreamAnalysis, TAB_SYSTEM),
            (S::MusicBrainz, TAB_SYSTEM),
            (S::Autoplay, TAB_MUSIC),
            (S::SearchPlay, TAB_MUSIC),
            (S::RadioSource, TAB_MUSIC),
            (S::NewShareBy(v::RadioKind::Mix), TAB_MUSIC),
            (S::DislikeButton, TAB_MUSIC),
            (S::Marks, TAB_MUSIC),
            (S::ListShuffle(v::ListKind::OwnPlaylist), TAB_MUSIC),
            (S::AlbumOpens, TAB_MUSIC),
            (S::AlbumVersions, TAB_MUSIC),
            (S::PlayerButton(v::PlayerButton::Sound), TAB_LOOK),
            (S::NavRoot, TAB_LOOK),
            (S::ApiEnabled, TAB_NET),
            (S::ApiTokens, TAB_NET),
            (S::MeshKeepMasters, TAB_NET),
            (S::MeshDevices, TAB_NET),
        ] {
            assert_eq!(id.tab(), tab, "{id:?}");
        }
        assert!(stage_tab(TAB_LYRICS) && stage_tab(TAB_LOOK) && !stage_tab(TAB_SYSTEM));
    }

    /// A link to a developer row in a handout build, or to its group, goes
    /// to a row users see; every other row is its own neighbour.
    #[test]
    fn no_link_ends_on_a_developer_row() {
        for &id in crate::screens::sweep::ALL {
            let n = id.user_neighbour();
            assert_ne!(n.level(), Level::Dev, "{id:?} → {n:?}");
            if id.level() != Level::Dev {
                assert_eq!(n, id);
            }
        }
        assert_eq!(SettingId::LyricsUi.user_neighbour(), SettingId::LyricsStyle);
        assert_eq!(SettingId::LyricsGpu.user_neighbour(), SettingId::LyricsModels);
    }

    /// «Ненастоящий FLAC» reads and writes the three old prefs, and nothing
    /// starts out making MP3 by itself.
    #[test]
    fn fake_flac_is_one_choice_over_three_prefs() {
        let id = SettingId::FakeFlac;
        for p in [Prefs::default(), Prefs::first_start()] {
            assert_ne!(id.chosen(&p), 3);
        }
        let mut p = Prefs::default();
        id.choose(&mut p, 3);
        assert!(p.check_fake_lossless && p.convert_fake_lossless && p.fake_flac_auto);
        id.choose(&mut p, 0);
        assert!(!p.check_fake_lossless);
        assert_eq!(id.chosen(&p), 0);
        id.choose(&mut p, 1);
        assert!(p.check_fake_lossless && !p.convert_fake_lossless);
        id.choose(&mut p, 2);
        assert!(p.check_fake_lossless && p.convert_fake_lossless && !p.fake_flac_auto);
    }

    #[test]
    fn every_choice_round_trips_through_prefs() {
        use SettingId as S;
        for id in [
            S::Language, S::Theme, S::Contrast, S::Scheme, S::CoverShape, S::CoverDir, S::AlbumDate, S::RingStyle, S::Ambient,
            S::BgShapes, S::BgColors, S::LyricsStyle, S::LyricsUnit, S::WaveFallback, S::LyricsGap, S::NpMotion, S::DblClickShape,
            S::OpusBitrate, S::NpVolume, S::LikeDownload, S::AlbumLike, S::SingleLike, S::LikedAlbumsPlace, S::PlaylistSync, S::RadioNewShare, S::RadioSource,
            S::NewShareBy(v::RadioKind::Track), S::NewShareBy(v::RadioKind::Autoplay), S::ProfileNames, S::BackgroundCards,
            S::RadioNoRepeat, S::RadioExplore, S::RadioUnheard, S::SoundNeural, S::SoundDevice, S::MixCardsWave, S::LyricsGpu, S::LyricsUi, S::ToastTime, S::LyricsStreamKeep,
            S::UpgradeEvery, S::UpgradeBar, S::UpgradeAuto, S::FakeFlac, S::ListShuffle(v::ListKind::Mix),
            S::ListShuffle(v::ListKind::Album), S::AlbumOpens, S::SearchPlay,
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
    fn language_setting_uses_prefs_only_and_exposes_native_endonyms() {
        use nikgob_core::i18n::{LangPref, Locale};
        let id = SettingId::Language;
        assert_eq!(id.tab(), TAB_LOOK);
        assert_eq!(id.level(), Level::Simple);
        assert_eq!(id.label_in(Locale::Ru), "Язык");
        assert_eq!(id.label_in(Locale::En), "Language");
        let ru_labels: Vec<&str> = id.choices_in(Locale::Ru).iter().map(|c| c.label).collect();
        let en_labels: Vec<&str> = id.choices_in(Locale::En).iter().map(|c| c.label).collect();
        assert_eq!(ru_labels, ["Системный", "Русский", "English (beta)"]);
        assert_eq!(en_labels, ["System", "Русский", "English (beta)"]);

        let before_global = nikgob_core::i18n::locale();
        for (i, pref) in LangPref::ALL.iter().enumerate() {
            let mut p = Prefs::default();
            id.choose(&mut p, i);
            assert_eq!(p.language, *pref);
            assert_eq!(id.chosen(&p), i);
        }
        assert_eq!(nikgob_core::i18n::locale(), before_global);
    }

    /// A preset row's options are its presets: each one picked reads back,
    /// values of the user's own read as «Свои» (the button after them), and
    /// its card's table names the values as the rows do.
    #[test]
    fn presets_are_choices_and_their_cards_read() {
        use v::presets::{Area, Field, FieldValue};
        for area in Area::ALL.into_iter().filter(|a| !a.names().is_empty()) {
            let id = SettingId::Preset(area);
            assert_eq!(id.choices().len(), area.names().len(), "{area:?}");
            for (i, c) in id.choices().iter().enumerate() {
                assert_eq!(c.label, area.names()[i]);
                let mut p = Prefs::default();
                id.choose(&mut p, i);
                assert_eq!(id.chosen(&p), i, "{area:?} {i}");
            }
            assert_eq!(id.level(), Level::Simple);
            assert_eq!(preset_title(area), id.label());
            assert!(matches!(base_body(id), Body::Table));
        }
        let mut own = Prefs::first_start();
        own.fps_limit = 75;
        assert_eq!(SettingId::Preset(Area::Load).chosen(&own), 3, "«Свои»");
        assert_eq!(field_text(Field::SeedRatio, &FieldValue::F32(0.0)), "без предела");
        assert_eq!(field_text(Field::SeedLimitGb, &FieldValue::U32(0)), "без лимита");
        assert_eq!(field_text(Field::SeedRatio, &FieldValue::F32(3.5)), "3,5");
        assert_eq!(field_text(Field::Ambient, &FieldValue::Ambient(v::Ambient::Reactive)), "Под звук");
        assert_eq!(field_text(Field::CoverShape, &FieldValue::Shape(v::CoverShape::Original)), "Квадрат");
        assert_eq!(field_text(Field::LyricsStyle, &FieldValue::U8(2)), "Волна");
        assert_eq!(field_text(Field::LyricsUnit, &FieldValue::U8(nikgob_view::LYRICS_BY_SYLLABLES)), "Слоги");
        assert_eq!(field_text(Field::LyricsLift, &FieldValue::F32(0.0)), "0 %");
        assert_eq!(field_text(Field::LyricsLightSpeed, &FieldValue::F32(0.55)), "55 %");
    }

    /// The new knobs of the track screen know their stage part, and the
    /// scenes acted out come from the rows that need them.
    #[test]
    fn rows_say_what_the_stage_shows() {
        use SettingId as S;
        let p = Prefs::default();
        for id in [S::CoverSpinSpeed, S::CoverPulseStrength] {
            assert_eq!((id.scene(), id.part()), (Some(Scene::Track), Some(Part::Cover)), "{id:?}");
        }
        assert_eq!((S::RingStyle.scene(), S::RingStyle.part()), (Some(Scene::Track), Some(Part::Ring)));
        assert_eq!((S::Ring.level(), S::RingStyle.level()), (Level::Simple, Level::Deep));
        assert_eq!(S::HomeSections.scene(), Some(Scene::Home));
        assert_eq!(S::MixesProfiles.part(), Some(Part::Home(Section::Mixes)));
        assert_eq!(S::AlbumDate.scene(), Some(Scene::Album));
        assert_eq!(S::Theme.scene(), None, "the theme shows on any screen");
        assert_eq!(S::RailCompact.part(), None);
        let random = Prefs { cover_shape: v::CoverShape::Random, ..p.clone() };
        assert_eq!(S::CoverShape.script(&random), Script::RandomShape);
        assert_eq!(S::ShapeRange.script(&random), Script::RandomShape);
        let one = Prefs { cover_shape: v::CoverShape::shapes()[3], ..random.clone() };
        assert_eq!(S::ShapeRange.script(&one), Script::None, "a shape of the pool under the pointer stands still");
        assert_eq!(S::LyricsCascade.script(&p), Script::LyricsStep);
        assert_eq!(S::LyricsSoftDepth.script(&p), Script::EdgesScroll);
        assert_eq!(S::LyricsCountdown.script(&p), Script::Countdown);
        assert_eq!(S::LyricsGapSize.script(&p), Script::Interlude);
        assert_eq!(S::NpMotion.script(&p), Script::Open);
        // The stage shows a knob in the look that has it.
        let karaoke = Prefs { lyrics_style: 0, lyrics_gap_style: 0, ..p.clone() };
        assert_eq!(S::JumpSpeed.staged(&karaoke).lyrics_style, 2);
        assert_eq!(S::LyricsGapWaves.staged(&karaoke).lyrics_gap_style, 1);
        assert_eq!(S::LyricsDim.staged(&karaoke), karaoke, "a row the look has is shown as it is");
        assert_eq!(S::Preset(v::presets::Area::Track).parts().len(), 4);
        assert!(S::Theme.parts().is_empty(), "the whole screen: nothing outlined");
        // Pulse and the ring default on. Those rows pulse only while the
        // switch is on; off, the stage stays calm. A shape row never pulses.
        let calm = Prefs { cover_pulse: false, ring: false, ..Prefs::default() };
        assert!(!S::CoverShape.needs_beat(&calm) && !S::RingStyle.needs_beat(&calm), "a shape stays calm");
        assert!(!S::CoverShape.needs_beat(&Prefs::default()), "a shape stays calm even when pulse is on");
        assert!(!S::CoverPulse.needs_beat(&calm), "pulse off: calm");
        assert!(S::CoverPulse.needs_beat(&Prefs { cover_pulse: true, ..calm.clone() }));
        assert!(!S::CoverPulseStrength.needs_beat(&calm));
        assert!(S::CoverPulseStrength.needs_beat(&Prefs { cover_pulse: true, ..calm.clone() }));
        assert!(S::Ring.needs_beat(&Prefs { ring: true, ..calm.clone() }));
        assert!(!S::Ring.needs_beat(&calm));
        assert!(!S::RingStyle.needs_beat(&Prefs { ring: true, ..calm.clone() }), "a look stays calm");
        assert!(S::Ambient.needs_beat(&Prefs { ambient: v::Ambient::Reactive, ..calm.clone() }));
        assert!(!S::Ambient.needs_beat(&Prefs { ambient: v::Ambient::Glow, ..calm }));
    }

    /// The card opens next to its own ⓘ, not under the row. A group title's
    /// row is the whole list; anchoring there used to drop the card at the
    /// window's corner.
    #[test]
    fn cards_open_next_to_their_button() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let size = vec2(300.0, 200.0);
        let button = Rect::from_min_size(pos2(120.0, 140.0), Vec2::splat(22.0));
        assert_eq!(place(button, size, screen), pos2(button.left(), button.bottom() + GAP), "just under the ⓘ");
        // Near the bottom: above the button, still on its left edge.
        let low = Rect::from_min_size(pos2(120.0, 700.0), Vec2::splat(22.0));
        let p = place(low, size, screen);
        assert!(p.y + size.y <= low.top() + 0.01, "flips above, {p:?}");
        assert!((p.x - low.left()).abs() < 0.01, "stays on the ⓘ, {p:?}");
        // No room to the right: the card grows left, still touching the ⓘ.
        let edge = Rect::from_min_size(pos2(1100.0, 140.0), Vec2::splat(22.0));
        let p = place(edge, size, screen);
        assert!(p.x + size.x <= screen.right() - 8.0 + 0.01, "on screen, {p:?}");
        assert!((p.x + size.x - edge.right()).abs() < 0.01, "flips left onto the ⓘ, {p:?}");
        assert!((p.y - (edge.bottom() + GAP)).abs() < 0.01, "still just under it, {p:?}");
        // A ⓘ in the bottom-right corner flips above and left, next to itself.
        let corner = Rect::from_min_size(pos2(1160.0, 760.0), Vec2::splat(22.0));
        let p = place(corner, size, screen);
        assert!(p.y + size.y <= corner.top() + 0.01, "above the corner ⓘ, {p:?}");
        assert!((p.x + size.x - corner.right()).abs() < 0.01, "left onto the corner ⓘ, {p:?}");
        assert!(p.x >= screen.left() && p.y >= screen.top());
    }
}
