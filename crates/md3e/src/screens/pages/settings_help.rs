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

use nikgob_view as v;
#[cfg(test)]
use nikgob_view::Prefs;

#[cfg(test)]
use egui::{Pos2, Rect, Vec2, pos2, vec2};
#[cfg(test)]
use nikgob_core::i18n::Locale;
#[cfg(test)]
use super::home::Section;
#[cfg(test)]
use crate::screens::stage::{Part, Scene, Script};

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
    /// «Копия с YouTube»: only by hand, or by itself when no source has it.
    YoutubeFallback,
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

/// Settings tabs, in the order of the tab switch.
pub const TAB_MUSIC: usize = 0;
pub const TAB_LYRICS: usize = 1;
pub const TAB_LOOK: usize = 2;
pub const TAB_SERVICES: usize = 3;
pub const TAB_DOWNLOADS: usize = 4;
pub const TAB_NET: usize = 5;
pub const TAB_SYSTEM: usize = 6;
/// Labels of the tab switch, in [`SettingId::tab`] order. In Russian on
/// purpose: sweep configs match the tabs by these names.
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

/// One option of a choice row: its label (as on the button) and what it does.
pub use nikgob_core::i18n::settings::ChoiceText as Choice;

pub use card::{
    begin_focus, drawn_rows, finish, group_start, help_button, help_button_beside, item, note_group,
    report_choices, report_control, report_menu, BTN, DrawnRow,
};

pub use catalog::{radio_source_line_of, radio_source_of, radio_source_patches};

pub(crate) use preview::beat;

pub use stage::preset_rows;

#[cfg(test)]
use card::{base_body, Body, GAP, place, FOCUS_S, GROUP_S, glow_at, group_tint_at};
#[cfg(test)]
use catalog::preset_title;

#[cfg(test)]
use stage::field_text_in;

mod card;
mod catalog;
mod preview;
mod stage;
#[cfg(test)]
mod tests;
