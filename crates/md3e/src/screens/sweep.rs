//! What the settings sweep needs to know about the settings page: every
//! row ([`SettingId`]), the values worth trying on it, what a value writes,
//! what makes the row shown and active, and the screens it changes. The
//! app's `--sweep-list` / `--sweep` runs (crates/app/src/sweep.rs) shoot
//! every (setting, value) with it for review agents that only see pictures.
//!
//! As much as can be is read from the settings code itself: the choices
//! from [`SettingId::choices`] / [`SettingId::choose`], the pref fields a
//! value writes from what it changes, the rows' labels, groups and which
//! rows a value shows, hides or fades from the page as it draws ([`Survey`],
//! a headless pass over the real page). What a switch or a slider writes
//! and its range are said here once, in [`spec`]: its match has no
//! catch-all, so a new [`SettingId`] does not build until it says what it
//! is, and [`ALL`] lists it (a test holds the two together).

use egui::{Context, Rect, pos2, vec2};
use nikgob_view::{self as v, Command, CoverShape, NpSlot, Prefs, Route, ViewState};

use super::pages::settings_help;
pub use super::pages::SettingId;
pub use super::pages::settings_help::{DrawnRow, Level, TABS};
use super::{Md3eUi, PaneTab};
use nikgob_view::presets::Area;

/// How a row is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Switch,
    /// Segmented buttons.
    Segments,
    /// A chip that opens a menu.
    Menu,
    Slider,
    /// A text field (or fields) and its «Сохранить».
    Field,
    /// Buttons only («Открыть», «Вернуть», a sign-in).
    Action,
    /// A line of text, nothing to set.
    Info,
    /// A list with «Снять» / «Вернуть» on each line.
    List,
    /// A grid of cover shapes.
    Shapes,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Switch => "switch",
            Self::Segments => "segments",
            Self::Menu => "menu",
            Self::Slider => "slider",
            Self::Field => "field",
            Self::Action => "action",
            Self::Info => "info",
            Self::List => "list",
            Self::Shapes => "shapes",
        }
    }
}

/// What the row writes, and how.
enum Spec {
    /// Nothing the sweep can set: why.
    Fixed(Kind, &'static str),
    Switch(fn(&mut Prefs) -> &mut bool),
    /// [`SettingId::choices`] through [`SettingId::choose`].
    Choice(Kind),
    /// A slider over `lo..=hi` in steps of `step`.
    Slider(fn(&mut Prefs) -> &mut f32, f32, f32, f32),
    /// The same over a whole number.
    Count(fn(&mut Prefs) -> &mut u32, f32, f32, f32),
    /// A speed limit in KiB/s (0: none, the right end).
    Speed(fn(&mut Prefs) -> &mut u32),
    /// One line of «Откуда качать»: its switch.
    Source(v::SourceKey),
    ShapeRange,
    ShapePick,
    UserName,
    OutputDevice,
    /// «Разделы главной»: a few orders and sections left out.
    HomeSections,
}

/// A screen a setting changes, as the sweep shoots it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Screen {
    /// Now Playing, the cover alone, playing.
    Np,
    /// The same, paused (the cover's square, the ring and the background at rest).
    NpPaused,
    /// Now Playing opening from the player bar, a few frames in.
    NpOpening,
    /// Now Playing with the lyrics beside the cover, mid-line.
    NpLyrics,
    /// …in a long pause between lines (the pause indicator).
    NpLyricsGap,
    /// …in the last seconds of that pause (the 3-2-1 countdown).
    NpLyricsCountdown,
    /// Now Playing with the queue beside the cover.
    NpQueue,
    /// A page with the lyrics in the side pane.
    LyricsPane,
    /// What the control under the lyrics opens (menu, window or page).
    LyricsUi,
    Home,
    Library,
    Album,
}

impl Screen {
    pub const ALL: [Self; 12] = [
        Self::Np,
        Self::NpPaused,
        Self::NpOpening,
        Self::NpLyrics,
        Self::NpLyricsGap,
        Self::NpLyricsCountdown,
        Self::NpQueue,
        Self::LyricsPane,
        Self::LyricsUi,
        Self::Home,
        Self::Library,
        Self::Album,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Np => "np",
            Self::NpPaused => "np_paused",
            Self::NpOpening => "np_opening",
            Self::NpLyrics => "np_lyrics",
            Self::NpLyricsGap => "np_lyrics_gap",
            Self::NpLyricsCountdown => "np_lyrics_countdown",
            Self::NpQueue => "np_queue",
            Self::LyricsPane => "lyrics_pane",
            Self::LyricsUi => "lyrics_ui",
            Self::Home => "home",
            Self::Library => "library",
            Self::Album => "album",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// What is on it, for the agents reading the pictures.
    pub fn about(self) -> &'static str {
        match self {
            Self::Np => "Now Playing (cover, ring, background, progress bar), playing",
            Self::NpPaused => "Now Playing paused: the cover morphs to its rest shape, ring and background calm down",
            Self::NpOpening => "Now Playing opening from the player bar: frames at about 0.1, 0.25, 0.45 s and settled",
            Self::NpLyrics => "Now Playing with synced lyrics beside the cover, in the middle of a sung line",
            Self::NpLyricsGap => "Now Playing lyrics during a long instrumental pause (the pause indicator ring)",
            Self::NpLyricsCountdown => "Now Playing lyrics in the last 3 s of a pause (3-2-1 countdown when it is on)",
            Self::NpQueue => "Now Playing with the queue beside the cover",
            Self::LyricsPane => "Главная with the lyrics in the right side pane",
            Self::LyricsUi => "Now Playing lyrics with what their control opens (menu, window or page, per «Где делать подсветку»)",
            Self::Home => "Главная (home page) with the player bar and the navigation rail",
            Self::Library => "Библиотека (library) page",
            Self::Album => "a library album page",
        }
    }

    /// Now Playing is open on it.
    pub fn now_playing(self) -> bool {
        matches!(
            self,
            Self::Np | Self::NpPaused | Self::NpOpening | Self::NpLyrics | Self::NpLyricsGap | Self::NpLyricsCountdown | Self::NpQueue
        )
    }
}

/// One value to try on a row.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    /// For paths: `0`, `on`, `1.5`, `Cookie9`.
    pub key: String,
    /// As the row shows it (a choice's label), or the number.
    pub label: String,
    /// A choice's own explanation.
    pub help: String,
}

impl Value {
    fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self { key: key.into(), label: label.into(), help: String::new() }
    }
}

use v::ListKind as L;
use v::PlayerButton as B;
use v::RadioKind as K;
use v::SourceKey as Src;
const KINDS: [K; 4] = [K::Track, K::Artist, K::Mix, K::Autoplay];

/// Every row, in the order of the tabs and pages.
pub const ALL: &[SettingId] = {
    use SettingId::*;
    &[
        // Музыка.
        OutputDevice, Gapless, Crossfade, Normalization, Equalizer, TrackSpeed, KeepTimbre, Autoplay, AutoplayWorld, SearchPlay, RadioNoRepeat,
        ListShuffle(L::Mix), ListShuffle(L::OwnPlaylist), ListShuffle(L::Album), ListShuffle(L::ReadyPlaylist), ShuffleRadio,
        Preset(Area::Recs), MixesDownloadedOnly, RadioNewShare, NewShareBy(K::Track), NewShareBy(K::Artist), NewShareBy(K::Mix), NewShareBy(K::Autoplay),
        MixesWithNew, RadioExplore, RadioUnheard, GraphUse, SoundNeural, SoundDevice, MixCardsWave, DislikeButton, DislikeSkips, DislikeSkipEverywhere, Marks, AlbumOpens,
        AlbumVersions, RadioSource,
        // Тексты.
        Preset(Area::Lyrics), LyricsStyle, LyricsUnit, LyricsSmooth, LightSpeed, WaveJump, JumpSpeed, LyricsLift, LyricsScale, LyricsLead,
        LyricsDim, LyricsCascade,
        LyricsGlow, LyricsSoftEdges, LyricsSoftDepth, LyricsSoftStrength, LyricsSoftFade, LyricsGap, LyricsGapWaves, LyricsGapHeight,
        LyricsGapSize, LyricsCountdown, WaveFallback, LyricsReturn, LyricsGenius, SaveLyrics, LyricsCheck, LyricsWordsPlaying,
        LyricsLinesAuto, LyricsStreams, LyricsStreamKeep, LyricsRecognize, LyricsModels,
        // Вид.
        Theme, Scheme, Contrast, Preset(Area::Track), SettingId::CoverShape, ShapeRange, ShapePick, CoverSpin, CoverDir,
        CoverSpinSpeed, CoverPulse, CoverPulseStrength, CoverPauseMorph, Ring, RingStyle, RingSpin, RingDir, RingSpinSpeed, RingBeat, RingPause,
        Ambient, AmbientStrength, BgShapes, BgCount, BgSize, BgOpacity, BgSpeed, BgReact, BgColors, BgHue, ProgressWave, NpVolume,
        NpVolumeHold, CoverClickPause, DblClickShape, TapWait, NpMotion, PlayerButton(B::Shuffle), PlayerButton(B::Repeat),
        PlayerButton(B::Like), PlayerButton(B::AddToPlaylist), PlayerButton(B::Queue), PlayerButton(B::Lyrics), PlayerButton(B::Sound),
        HomeSections, MixesProfiles, MixesEverything, MixesMoods, MixesOnRepeat, BackgroundCards, ProfileNames, HiddenMixes, AlbumDate,
        LikedAlbumsPlace, UiScale, RailCompact, NavRoot, ReducedMotion, Tooltips, TrackMenu, PaneLyrics, PaneInfo,
        // Сервисы.
        SpotifyEnabled, Spotify, SpotifyPersonal, SpotifyPlayback, SpotifySoloist, ImportSpotify, YouTubeEnabled, YouTube,
        YouTubeCookies, ImportYouTube, SoundCloudEnabled, SoundCloud, SoundCloudToken, ImportSoundCloud, DeezerEnabled, PushLikes,
        AlbumLike, SingleLike, PlaylistSync, ImportFiles, ImportSavedSongs, StreamCache,
        // Загрузки.
        Preset(Area::Sources), DownloadSource(Src::Torrents), DownloadSource(Src::Khinsider), DownloadSource(Src::Soulseek),
        DownloadSource(Src::Qobuz),
        DownloadSource(Src::Tidal), DownloadSource(Src::Squidify), DownloadSource(Src::Archive), DownloadSource(Src::Bandcamp),
        LikeDownload, UpgradeBar, UpgradeEvery, OpusBitrate, UpgradeAuto, FakeFlac, FakeRestore, Soulseek, SoulseekShare, Prowlarr,
        Preset(Area::Seeding), NoSeed, SeedLimit, SeedDays, SeedRatio, DownloadLimit, UploadLimit, SoulseekUpload,
        // Система.
        UserName, Autostart, AutostartHidden, Preset(Area::Load), FpsLimit, TrayKeepWindow, AutoDownloads, StreamAnalysis, LyricsSeparate, LyricsDeep,
        LyricsWordsAll, ToastTime, DownloadToasts, LibraryDir, MastersDir, Inbox, Trash, ExportData,
        // Для разработчика.
        LyricsUi, LyricsGpu, LyricsModelsAuto, MusicBrainz, AcoustId, LastfmKey, ListenbrainzToken, BackgroundCountsLess,
        ProfilesSplit, GraphServices, GraphOpenData, GraphPersonal, LibraryAsTaste, DislikeInRecs, DemoProfile, FirstRun,
    ]
};

/// The row's name for files and plans: `CoverShape`, `NewShareBy-Track`,
/// `DownloadSource-Qobuz`.
pub fn key(id: SettingId) -> String {
    format!("{id:?}").replace('(', "-").replace(')', "")
}

pub fn parse(key_: &str) -> Option<SettingId> {
    ALL.iter().copied().find(|id| key(*id) == key_)
}

/// The tab's name.
pub fn tab_name(id: SettingId) -> &'static str {
    TABS[id.tab().min(TABS.len() - 1)]
}

pub fn help(id: SettingId) -> &'static str {
    id.help()
}

pub fn beta(id: SettingId) -> bool {
    id.beta()
}

/// The row's name as the page shows it.
pub fn label(id: SettingId) -> &'static str {
    id.label()
}

/// When the row shows: «Просто», «Подробно» or «Для разработчика».
pub fn level(id: SettingId) -> Level {
    id.level()
}

fn spec(id: SettingId) -> Spec {
    use Kind::{Action, Field, List, Menu, Segments};
    use SettingId as S;
    const ACCOUNT: &str = "a service account (sign-in); shown with whatever the data dir has";
    const SECRET: &str = "kept in settings.toml, not in the prefs; typed in, never set by the sweep";
    const BUTTONS: &str = "buttons only; the sweep never presses them (they open folders, sign in or change files)";
    match id {
        S::Theme | S::Contrast | S::NpVolume | S::CoverShape | S::CoverDir | S::RingDir | S::RingStyle | S::Ambient | S::BgColors
        | S::LyricsStyle | S::LyricsUnit | S::NpMotion | S::ToastTime | S::LyricsUi | S::LyricsGpu | S::LyricsStreamKeep
        | S::OpusBitrate | S::RadioNewShare | S::ProfileNames | S::LikeDownload | S::AlbumLike | S::SingleLike
        | S::LikedAlbumsPlace | S::UpgradeBar | S::UpgradeAuto | S::PlaylistSync | S::AlbumDate => Spec::Choice(Segments),
        // Swatches, picked like segments.
        S::BgHue => Spec::Choice(Segments),
        S::Scheme | S::WaveFallback | S::LyricsGap | S::DblClickShape | S::NewShareBy(_) | S::RadioNoRepeat | S::RadioExplore
        | S::RadioUnheard | S::BackgroundCards | S::BgShapes | S::UpgradeEvery | S::FakeFlac | S::ListShuffle(_) | S::AlbumOpens
        | S::SearchPlay => Spec::Choice(Menu),
        S::SoundNeural => Spec::Fixed(Menu, "a mode downloads the neural sound model; the sweep leaves it as it is"),
        S::SoundDevice => Spec::Fixed(Menu, "open only with the neural sound analysis on, which downloads its model"),
        S::MixCardsWave => Spec::Choice(Menu),
        S::RadioSource => Spec::Fixed(Segments, "chosen in the queue's radio header; no row in Settings"),
        // A preset: its presets as choices (no «Свои»: it only takes back).
        S::Preset(a) if a.names().is_empty() => Spec::Fixed(Segments, "no presets yet (TODO 36); the row is not drawn"),
        S::Preset(_) => Spec::Choice(Segments),
        S::CoverSpinSpeed => Spec::Slider(|p| &mut p.cover_spin_s, 12.0, 90.0, 1.0),
        S::CoverPulseStrength => Spec::Slider(|p| &mut p.cover_pulse_strength, 0.02, 0.15, 0.01),
        S::RingSpinSpeed => Spec::Slider(|p| &mut p.ring_spin_s, 10.0, 90.0, 1.0),
        S::HomeSections => Spec::HomeSections,
        S::ProgressWave => Spec::Switch(|p| &mut p.progress_wave),
        S::NpVolumeHold => Spec::Slider(|p| &mut p.np_volume_pop_hold, 0.3, 3.0, 0.1),
        S::ShapeRange => Spec::ShapeRange,
        S::ShapePick => Spec::ShapePick,
        S::CoverPauseMorph => Spec::Switch(|p| &mut p.cover_pause_morph),
        S::CoverSpin => Spec::Switch(|p| &mut p.cover_spin),
        S::CoverPulse => Spec::Switch(|p| &mut p.cover_pulse),
        S::Ring => Spec::Switch(|p| &mut p.ring),
        S::RingSpin => Spec::Switch(|p| &mut p.spin_ring),
        S::RingBeat => Spec::Switch(|p| &mut p.ring_beat),
        S::RingPause => Spec::Switch(|p| &mut p.ring_on_pause),
        S::AmbientStrength => Spec::Slider(|p| &mut p.ambient_strength, 0.25, 2.0, 0.05),
        S::BgCount => Spec::Count(|p| &mut p.bg_shapes_count, 4.0, 32.0, 1.0),
        S::BgSize => Spec::Slider(|p| &mut p.bg_shapes_size, 0.5, 2.0, 0.05),
        S::BgOpacity => Spec::Slider(|p| &mut p.bg_shapes_opacity, 0.25, 2.0, 0.05),
        S::BgSpeed => Spec::Slider(|p| &mut p.bg_shapes_speed, 0.25, 3.0, 0.05),
        S::BgReact => Spec::Slider(|p| &mut p.bg_shapes_react, 0.0, 2.0, 0.05),
        S::WaveJump => Spec::Slider(|p| &mut p.lyrics_wave_jump, 0.0, 2.0, 0.1),
        S::JumpSpeed => Spec::Slider(|p| &mut p.lyrics_jump_speed, 0.5, 2.0, 0.05),
        S::LyricsLift => Spec::Slider(|p| &mut p.lyrics_lift, 0.0, 1.0, 0.05),
        S::LightSpeed => Spec::Slider(|p| &mut p.lyrics_light_speed, 0.0, 1.0, 0.05),
        S::LyricsSmooth => Spec::Slider(|p| &mut p.lyrics_smooth, 0.0, 1.0, 0.05),
        S::LyricsDim => Spec::Slider(|p| &mut p.lyrics_dim, 0.0, 1.0, 0.05),
        S::LyricsScale => Spec::Slider(|p| &mut p.lyrics_scale, 0.8, 1.4, 0.05),
        S::LyricsLead => Spec::Slider(|p| &mut p.lyrics_lead, -0.5, 0.5, 0.05),
        S::LyricsCascade => Spec::Switch(|p| &mut p.lyrics_cascade),
        S::LyricsGlow => Spec::Switch(|p| &mut p.lyrics_glow),
        S::LyricsGapWaves => Spec::Slider(|p| &mut p.lyrics_gap_waves, 3.0, 16.0, 1.0),
        S::LyricsGapHeight => Spec::Slider(|p| &mut p.lyrics_gap_height, 0.5, 2.0, 0.05),
        S::LyricsGapSize => Spec::Slider(|p| &mut p.lyrics_gap_size, 0.8, 1.5, 0.05),
        S::LyricsCountdown => Spec::Switch(|p| &mut p.lyrics_countdown),
        S::LyricsSoftEdges => Spec::Switch(|p| &mut p.lyrics_soft_edges),
        S::LyricsSoftStrength => Spec::Slider(|p| &mut p.lyrics_soft_strength, 0.0, 1.5, 0.05),
        S::LyricsSoftDepth => Spec::Slider(|p| &mut p.lyrics_soft_depth, 0.08, 0.4, 0.01),
        S::LyricsSoftFade => Spec::Slider(|p| &mut p.lyrics_soft_fade, 0.0, 1.0, 0.05),
        S::RailCompact => Spec::Switch(|p| &mut p.rail_compact),
        S::PaneLyrics => Spec::Switch(|p| &mut p.pane_lyrics),
        S::PaneInfo => Spec::Switch(|p| &mut p.pane_info),
        S::ReducedMotion => Spec::Switch(|p| &mut p.reduced_motion),
        S::Tooltips => Spec::Switch(|p| &mut p.tooltips),
        S::FpsLimit => Spec::Count(|p| &mut p.fps_limit, 30.0, 144.0, 1.0),
        // Основные.
        S::UserName => Spec::UserName,
        S::Autostart => Spec::Switch(|p| &mut p.autostart),
        S::AutostartHidden => Spec::Switch(|p| &mut p.autostart_hidden),
        S::TrayKeepWindow => Spec::Switch(|p| &mut p.tray_keep_window),
        S::CoverClickPause => Spec::Switch(|p| &mut p.cover_click_pause),
        S::TapWait => Spec::Slider(|p| &mut p.np_tap_wait, 0.0, 0.4, 0.02),
        S::SaveLyrics => Spec::Switch(|p| &mut p.save_found_lyrics),
        S::LyricsReturn => Spec::Slider(|p| &mut p.lyrics_return_s, 1.0, 15.0, 1.0),
        S::LyricsCheck => Spec::Switch(|p| &mut p.lyrics_check),
        S::LyricsLinesAuto => Spec::Switch(|p| &mut p.lyrics_lines_auto),
        S::LyricsWordsPlaying => Spec::Switch(|p| &mut p.lyrics_words_playing),
        S::LyricsWordsAll => Spec::Switch(|p| &mut p.lyrics_words_all),
        S::LyricsRecognize => Spec::Switch(|p| &mut p.lyrics_recognize),
        S::LyricsSeparate => Spec::Switch(|p| &mut p.lyrics_separate),
        S::LyricsDeep => Spec::Switch(|p| &mut p.lyrics_deep),
        S::LyricsModelsAuto => Spec::Switch(|p| &mut p.lyrics_models_auto),
        S::LyricsGenius => Spec::Switch(|p| &mut p.lyrics_genius),
        S::LyricsStreams => Spec::Switch(|p| &mut p.lyrics_streams),
        S::LyricsModels => Spec::Fixed(List, "a «Модели» row that opens the list; the sweep never opens it and never presses «Скачать» / delete"),
        S::ShuffleRadio => Spec::Switch(|p| &mut p.shuffle_radio),
        S::OutputDevice => Spec::OutputDevice,
        S::Gapless => Spec::Switch(|p| &mut p.gapless),
        S::Crossfade => Spec::Slider(|p| &mut p.crossfade_s, 0.0, 12.0, 1.0),
        S::Normalization => Spec::Switch(|p| &mut p.normalization),
        // Its switch; «Настроить» opens the sheet, which the sweep leaves shut.
        S::Equalizer => Spec::Switch(|p| &mut p.eq_on),
        S::TrackSpeed => Spec::Switch(|p| &mut p.speed_enabled),
        S::KeepTimbre => Spec::Switch(|p| &mut p.keep_timbre),
        S::LibraryDir | S::MastersDir | S::Inbox | S::ExportData | S::FakeRestore | S::DemoProfile | S::FirstRun => Spec::Fixed(Action, BUTTONS),
        S::Trash => Spec::Switch(|p| &mut p.use_trash),
        S::MusicBrainz | S::AcoustId | S::LastfmKey | S::ListenbrainzToken | S::Prowlarr | S::Soulseek | S::SpotifyPersonal
        | S::YouTubeCookies | S::SoundCloudToken => Spec::Fixed(Field, SECRET),
        // Рекомендации.
        S::MixesDownloadedOnly => Spec::Switch(|p| &mut p.mixes_downloaded_only),
        S::MixesWithNew => Spec::Switch(|p| &mut p.mixes_with_new),
        S::Autoplay => Spec::Switch(|p| &mut p.autoplay),
        S::AutoplayWorld => Spec::Switch(|p| &mut p.autoplay_continue_world),
        S::BackgroundCountsLess => Spec::Switch(|p| &mut p.background_counts_less),
        S::ProfilesSplit => Spec::Switch(|p| &mut p.profiles_split),
        S::MixesProfiles => Spec::Switch(|p| &mut p.mixes_profiles),
        S::MixesEverything => Spec::Switch(|p| &mut p.mixes_everything),
        S::MixesMoods => Spec::Switch(|p| &mut p.mixes_moods),
        S::MixesOnRepeat => Spec::Switch(|p| &mut p.mixes_on_repeat),
        S::DislikeButton => Spec::Switch(|p| &mut p.dislike_button),
        S::DislikeSkips => Spec::Switch(|p| &mut p.dislike_skips),
        S::DislikeSkipEverywhere => Spec::Switch(|p| &mut p.dislike_skip_everywhere),
        S::DislikeInRecs => Spec::Switch(|p| &mut p.dislike_in_recs),
        S::Marks => Spec::Fixed(Action, "a row whose button opens the page of the marks; the sweep never opens it"),
        S::PlayerButton(B::Sound) => Spec::Switch(|p| &mut p.player_buttons.sound),
        S::PlayerButton(B::Shuffle) => Spec::Switch(|p| &mut p.player_buttons.shuffle),
        S::PlayerButton(B::Repeat) => Spec::Switch(|p| &mut p.player_buttons.repeat),
        S::PlayerButton(B::Like) => Spec::Switch(|p| &mut p.player_buttons.like),
        S::PlayerButton(B::AddToPlaylist) => Spec::Switch(|p| &mut p.player_buttons.add_to_playlist),
        S::PlayerButton(B::Queue) => Spec::Switch(|p| &mut p.player_buttons.queue),
        S::PlayerButton(B::Lyrics) => Spec::Switch(|p| &mut p.player_buttons.lyrics),
        S::NavRoot => Spec::Switch(|p| &mut p.nav_root),
        S::AlbumVersions => Spec::Switch(|p| &mut p.album_other_versions),
        S::UiScale => Spec::Slider(|p| &mut p.ui_scale, 0.8, 1.5, 0.1),
        S::TrackMenu => Spec::Fixed(Action, "a row that opens the list of the track menu's items; the sweep never opens it"),
        S::HiddenMixes => Spec::Fixed(List, "shown only while a mix is hidden from Главная (⋮ → «Не предлагать этот микс»)"),
        S::GraphUse => Spec::Switch(|p| &mut p.graph_use),
        S::GraphServices => Spec::Switch(|p| &mut p.graph_services),
        S::GraphOpenData => Spec::Switch(|p| &mut p.graph_open_data),
        S::GraphPersonal => Spec::Switch(|p| &mut p.graph_personal),
        S::LibraryAsTaste => Spec::Switch(|p| &mut p.library_as_taste),
        // Источники.
        S::SpotifyEnabled => Spec::Switch(|p| &mut p.spotify_enabled),
        S::YouTubeEnabled => Spec::Switch(|p| &mut p.youtube_enabled),
        S::DeezerEnabled => Spec::Switch(|p| &mut p.deezer_enabled),
        S::SoundCloudEnabled => Spec::Switch(|p| &mut p.soundcloud_enabled),
        S::Spotify | S::YouTube | S::SoundCloud | S::SpotifyPlayback | S::SpotifySoloist => Spec::Fixed(Action, ACCOUNT),
        S::ImportSpotify | S::ImportYouTube | S::ImportSoundCloud | S::ImportFiles => Spec::Fixed(Action, BUTTONS),
        S::PushLikes => Spec::Switch(|p| &mut p.push_likes),
        S::AutoDownloads => Spec::Count(|p| &mut p.auto_download_limit, 1.0, 8.0, 1.0),
        S::DownloadToasts => Spec::Switch(|p| &mut p.download_toasts),
        S::DownloadSource(k) => Spec::Source(k),
        S::ImportSavedSongs => Spec::Switch(|p| &mut p.import_saved_songs),
        S::StreamCache => Spec::Count(|p| &mut p.stream_cache_mb, 0.0, 4000.0, 100.0),
        S::StreamAnalysis => Spec::Switch(|p| &mut p.stream_analysis),
        S::SoulseekShare | S::SoulseekUpload => {
            Spec::Fixed(Kind::Switch, "kept in settings.toml and needs a Soulseek account; shown as the data dir has it")
        }
        S::NoSeed => Spec::Switch(|p| &mut p.no_seed),
        S::SeedLimit => Spec::Count(|p| &mut p.seed_limit_gb, 0.0, 500.0, 5.0),
        S::SeedDays => Spec::Count(|p| &mut p.seed_days, 1.0, 90.0, 1.0),
        S::SeedRatio => Spec::Slider(|p| &mut p.seed_ratio, 0.0, 5.0, 0.5),
        S::DownloadLimit => Spec::Speed(|p| &mut p.download_limit_kbps),
        S::UploadLimit => Spec::Speed(|p| &mut p.upload_limit_kbps),
    }
}

pub fn kind(id: SettingId) -> Kind {
    match spec(id) {
        Spec::Fixed(k, _) | Spec::Choice(k) => k,
        Spec::Switch(_) | Spec::Source(_) => Kind::Switch,
        Spec::Slider(..) | Spec::Count(..) | Spec::Speed(_) => Kind::Slider,
        Spec::ShapeRange | Spec::ShapePick => Kind::Shapes,
        Spec::UserName => Kind::Field,
        Spec::OutputDevice => Kind::Menu,
        Spec::HomeSections => Kind::List,
    }
}

/// Why the sweep has no values for the row, or why the row may be
/// missing (it needs something the data dir may not have).
pub fn needs(id: SettingId) -> Option<&'static str> {
    use SettingId as S;
    match (spec(id), id) {
        (Spec::Fixed(_, why), _) => Some(why),
        (_, S::LyricsGpu) if !cfg!(windows) => Some("Windows only (DirectML)"),
        (_, id) if id.level() == Level::Dev => Some("«Для разработчика»: drawn in «Подробно» of the owner's own builds only"),
        (_, S::DownloadSource(_)) => Some("drawn only for the sources the runtime lists as available"),
        (_, S::Gapless | S::Crossfade) => Some("faded while Spotify Soloist plays"),
        _ => None,
    }
}

/// Numbers a slider is tried at: both ends, the middle and two between,
/// on its steps.
fn stops(lo: f32, hi: f32, step: f32) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::new();
    for k in 0..5 {
        let v = (((lo + (hi - lo) * k as f32 / 4.0) / step).round() * step).clamp(lo, hi);
        if !out.iter().any(|x| (x - v).abs() < step * 0.5) {
            out.push(v);
        }
    }
    out
}

/// `1.5`, `0.25`, `144`, `-0.5`.
pub fn num_key(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

const SPEEDS: [u32; 5] = [50, 500, 2_000, 10_000, 0];
const NAMES: [&str; 3] = ["", "Ник", "Константин Константинопольский"];

/// «Разделы главной» as the sweep tries it: as it comes, without «Миксы для
/// тебя», upside down.
fn home_orders() -> Vec<(&'static str, &'static str, Vec<v::HomeSectionSwitch>)> {
    let all = |off: Option<v::HomeSection>| {
        v::HomeSection::ALL.iter().map(|k| v::HomeSectionSwitch { key: *k, on: Some(*k) != off }).collect::<Vec<_>>()
    };
    let mut upside_down = all(None);
    upside_down.reverse();
    vec![
        ("all", "Все по порядку", Vec::new()),
        ("no-mixes", "Без «Миксов для тебя»", all(Some(v::HomeSection::Mixes))),
        ("reversed", "Снизу вверх", upside_down),
    ]
}

fn shape_sets() -> Vec<(&'static str, String, Vec<CoverShape>)> {
    let all = CoverShape::shapes();
    vec![
        ("all", "Все фигуры".to_owned(), Vec::new()),
        ("one", format!("Одна: {}", all[1].label()), vec![all[1]]),
        ("three", "Три".to_owned(), all.iter().copied().take(3).collect()),
    ]
}

/// The values to try on `id` (none for a row the sweep cannot set). `p`
/// matters only for the output devices it lists.
pub fn values(id: SettingId, p: &Prefs) -> Vec<Value> {
    match spec(id) {
        Spec::Fixed(..) => Vec::new(),
        Spec::Switch(_) | Spec::Source(_) => vec![Value::new("off", "Выкл"), Value::new("on", "Вкл")],
        Spec::Choice(_) => id
            .choices()
            .iter()
            .enumerate()
            .map(|(i, c)| Value { key: i.to_string(), label: c.label.to_owned(), help: c.help.to_owned() })
            .collect(),
        Spec::Slider(_, lo, hi, step) | Spec::Count(_, lo, hi, step) => {
            stops(lo, hi, step).into_iter().map(|v| Value::new(num_key(v), num_key(v))).collect()
        }
        Spec::Speed(_) => SPEEDS
            .iter()
            .map(|k| match k {
                0 => Value::new("unlimited", "без лимита"),
                k => Value::new(k.to_string(), format!("{k} КиБ/с")),
            })
            .collect(),
        Spec::ShapeRange => shape_sets().into_iter().map(|(k, l, _)| Value::new(k, l)).collect(),
        Spec::ShapePick => CoverShape::shapes().iter().map(|c| Value::new(format!("{c:?}"), c.label())).collect(),
        Spec::UserName => ["empty", "short", "long"].iter().zip(NAMES).map(|(k, n)| Value::new(*k, n)).collect(),
        Spec::OutputDevice => std::iter::once(Value::new("default", "Системное по умолчанию"))
            .chain(p.output_devices.iter().take(2).enumerate().map(|(i, d)| Value::new(format!("device-{}", i + 1), d.clone())))
            .collect(),
        Spec::HomeSections => home_orders().into_iter().map(|(k, l, _)| Value::new(k, l)).collect(),
    }
}

/// Sets value `i` of [`values`] in `p`; false when there is none.
pub fn apply(id: SettingId, i: usize, p: &mut Prefs) -> bool {
    match spec(id) {
        Spec::Fixed(..) => return false,
        Spec::Switch(_) | Spec::Source(_) if i > 1 => return false,
        Spec::Switch(get) => *get(p) = i == 1,
        Spec::Choice(_) => {
            if i >= id.choices().len() {
                return false;
            }
            id.choose(p, i);
        }
        Spec::Slider(get, lo, hi, step) => match stops(lo, hi, step).get(i) {
            Some(v) => *get(p) = *v,
            None => return false,
        },
        Spec::Count(get, lo, hi, step) => match stops(lo, hi, step).get(i) {
            Some(v) => *get(p) = v.round() as u32,
            None => return false,
        },
        Spec::Speed(get) => match SPEEDS.get(i) {
            Some(v) => *get(p) = *v,
            None => return false,
        },
        Spec::Source(key) => {
            let mut order = p.source_order();
            if let Some(s) = order.iter_mut().find(|s| s.key == key) {
                s.on = i == 1;
            }
            p.download_sources = order;
        }
        Spec::ShapeRange => match shape_sets().into_iter().nth(i) {
            Some((_, _, set)) => p.random_shapes = set,
            None => return false,
        },
        Spec::ShapePick => match CoverShape::shapes().get(i) {
            Some(c) => p.cover_shape = *c,
            None => return false,
        },
        Spec::UserName => match NAMES.get(i) {
            Some(n) => v::PrefPatch::UserName((*n).to_owned()).apply(p),
            None => return false,
        },
        Spec::OutputDevice => match i {
            0 => p.output_device = None,
            n => match p.output_devices.get(n - 1) {
                Some(d) => p.output_device = Some(d.clone()),
                None => return false,
            },
        },
        Spec::HomeSections => match home_orders().into_iter().nth(i) {
            Some((_, _, list)) => p.home_sections = list,
            None => return false,
        },
    }
    true
}

/// The value of [`values`] that `p` holds, as the row shows it (a choice:
/// the option lit), if it is one of them.
pub fn shown(id: SettingId, p: &Prefs) -> Option<usize> {
    if let Spec::Choice(_) = spec(id) {
        return Some(id.chosen(p));
    }
    (0..values(id, p).len()).find(|&i| {
        let mut q = p.clone();
        apply(id, i, &mut q) && q == *p
    })
}

/// Makes the row of `id` shown and active (its switch on, a shape that
/// turns): what the page needs before the row's values do anything.
pub fn enable(id: SettingId, p: &mut Prefs) {
    use SettingId as S;
    use v::{Ambient, BgShapes, RingStyle};
    let unsquare = |p: &mut Prefs| {
        if p.cover_shape == CoverShape::Original {
            p.cover_shape = CoverShape::default();
        }
    };
    let bg_on = |p: &mut Prefs| {
        if p.bg_shapes == BgShapes::Off {
            p.bg_shapes = BgShapes::Float;
        }
    };
    let ring_turns = |p: &mut Prefs| {
        p.ring = true;
        if p.ring_style == RingStyle::Bars {
            p.ring_style = RingStyle::Circle;
        }
    };
    match id {
        S::NpVolumeHold => (p.np_volume, p.np_volume_pop) = (false, true),
        S::KeepTimbre => p.speed_enabled = true,
        S::ShapeRange => p.cover_shape = CoverShape::Random,
        S::CoverSpin => unsquare(p),
        S::CoverDir | S::CoverSpinSpeed => {
            unsquare(p);
            p.cover_spin = true;
        }
        S::CoverPulseStrength => p.cover_pulse = true,
        S::RingStyle | S::RingBeat | S::RingPause => p.ring = true,
        S::RingSpin => ring_turns(p),
        S::RingDir | S::RingSpinSpeed => {
            ring_turns(p);
            p.spin_ring = true;
        }
        S::AmbientStrength => {
            if p.ambient == Ambient::Off {
                p.ambient = Ambient::Glow;
            }
        }
        S::BgCount | S::BgSize | S::BgOpacity | S::BgSpeed | S::BgReact | S::BgColors | S::BgHue => bg_on(p),
        S::LyricsSmooth | S::WaveFallback | S::LightSpeed => {
            if p.lyrics_style == 1 && p.lyrics_unit == v::LYRICS_BY_LETTERS {
                p.lyrics_unit = v::LYRICS_BY_SYLLABLES;
            }
        }
        S::WaveJump | S::JumpSpeed => p.lyrics_style = 2,
        S::LyricsLift => p.lyrics_style = 0,
        S::LyricsSoftStrength | S::LyricsSoftDepth | S::LyricsSoftFade => p.lyrics_soft_edges = true,
        S::LyricsGapWaves | S::LyricsGapHeight => {
            if !matches!(p.lyrics_gap_style, 1 | 3) {
                p.lyrics_gap_style = 1;
            }
        }
        S::AutostartHidden => p.autostart = true,
        S::TapWait => {
            if p.dbl_click_shape == 0 {
                p.dbl_click_shape = 1;
            }
        }
        S::LyricsStreamKeep => p.lyrics_streams = true,
        S::RadioNewShare | S::NewShareBy(_) | S::MixesWithNew => p.mixes_downloaded_only = false,
        S::RadioExplore => {
            p.mixes_downloaded_only = false;
            if KINDS.iter().all(|k| p.new_share_of(*k) == v::RadioNewShare::None) {
                v::PrefPatch::RadioNewShare(v::RadioNewShare::Some).apply(p);
            }
        }
        S::AutoplayWorld | S::RadioSource => p.autoplay = true,
        S::GraphServices | S::GraphOpenData | S::LastfmKey => p.graph_use = true,
        S::ListenbrainzToken => (p.graph_use, p.graph_open_data) = (true, true),
        S::UpgradeBar | S::UpgradeAuto => {
            if p.upgrade_every == v::UpgradeEvery::Never {
                p.upgrade_every = v::UpgradeEvery::ALL[1];
            }
        }
        S::SeedLimit | S::SeedDays | S::SeedRatio => p.no_seed = false,
        _ => {}
    }
}

/// The screens a value of `id` changes, besides the settings page itself.
pub fn screens(id: SettingId) -> Vec<Screen> {
    use Screen::*;
    use SettingId as S;
    match id {
        S::Theme | S::Contrast | S::Scheme => vec![Home, Album, Np, NpLyrics],
        S::ProgressWave => vec![Np, NpPaused, Home],
        S::NpVolume | S::NpVolumeHold | S::FpsLimit | S::DislikeButton | S::PlayerButton(_) => vec![Np],
        S::CoverShape | S::Ring | S::RingStyle | S::CoverPauseMorph => vec![Np, NpPaused, NpLyrics],
        S::AlbumDate => vec![Album],
        S::ShapeRange | S::ShapePick | S::Ambient | S::AmbientStrength | S::RingPause => vec![Np, NpPaused],
        S::CoverSpin | S::CoverDir | S::CoverPulse | S::RingSpin | S::RingDir | S::RingBeat => vec![Np],
        S::CoverSpinSpeed | S::CoverPulseStrength | S::RingSpinSpeed => vec![Np],
        S::Preset(Area::Track) => vec![Np, NpPaused],
        S::Preset(Area::Recs) | S::HomeSections => vec![Home],
        S::BgShapes | S::BgCount | S::BgSize | S::BgOpacity | S::BgSpeed | S::BgReact | S::BgColors | S::BgHue => vec![Np],
        S::Preset(Area::Lyrics) | S::LyricsStyle | S::LyricsUnit | S::WaveFallback | S::WaveJump | S::JumpSpeed | S::LyricsLift
        | S::LightSpeed | S::LyricsSmooth | S::LyricsDim | S::LyricsScale | S::LyricsLead | S::LyricsGlow | S::LyricsCascade | S::LyricsSoftEdges
        | S::LyricsSoftStrength | S::LyricsSoftDepth | S::LyricsSoftFade => vec![NpLyrics],
        S::LyricsGap | S::LyricsGapWaves | S::LyricsGapHeight | S::LyricsGapSize => vec![NpLyricsGap],
        S::LyricsCountdown => vec![NpLyricsCountdown],
        S::RailCompact => vec![Home, Library],
        S::UiScale => vec![Home, Np],
        S::PaneLyrics => vec![Home, LyricsPane],
        S::PaneInfo | S::UserName => vec![Home],
        S::NpMotion => vec![NpOpening],
        S::ReducedMotion => vec![NpOpening, Np, NpLyrics],
        S::LyricsUi => vec![LyricsUi],
        S::Autoplay | S::AutoplayWorld => vec![NpQueue],
        S::MixesDownloadedOnly | S::MixesProfiles | S::MixesEverything | S::MixesMoods | S::MixesOnRepeat
        | S::BackgroundCards | S::ProfileNames | S::ProfilesSplit | S::MixesWithNew => vec![Home],
        S::LikedAlbumsPlace => vec![Library],
        _ => Vec::new(),
    }
}

/// Its screens move: a few frames apart tell more than one.
pub fn animated(id: SettingId) -> bool {
    use SettingId as S;
    matches!(
        id,
        S::ProgressWave
            | S::CoverSpinSpeed
            | S::CoverPulseStrength
            | S::RingSpinSpeed
            | S::Preset(Area::Track)
            | S::Preset(Area::Lyrics)
            | S::LyricsLift
            | S::CoverSpin
            | S::CoverDir
            | S::CoverPulse
            | S::RingSpin
            | S::RingDir
            | S::RingBeat
            | S::Ambient
            | S::AmbientStrength
            | S::BgShapes
            | S::BgCount
            | S::BgSize
            | S::BgOpacity
            | S::BgSpeed
            | S::BgReact
            | S::LyricsStyle
            | S::LyricsUnit
            | S::WaveJump
            | S::JumpSpeed
            | S::LightSpeed
            | S::LyricsSmooth
            | S::LyricsCascade
            | S::LyricsGap
            | S::LyricsGapWaves
            | S::LyricsGapHeight
            | S::LyricsCountdown
            | S::ReducedMotion
            | S::FpsLimit
    )
}

/// A row every tab draws first («Просто» too), for opening the tab at its
/// top.
pub fn tab_first(tab: usize) -> SettingId {
    match tab {
        settings_help::TAB_MUSIC => SettingId::OutputDevice,
        settings_help::TAB_LYRICS => SettingId::LyricsStyle,
        settings_help::TAB_LOOK => SettingId::Theme,
        settings_help::TAB_SERVICES => SettingId::SpotifyEnabled,
        settings_help::TAB_DOWNLOADS => SettingId::Preset(Area::Sources),
        _ => SettingId::UserName,
    }
}

/// The settings rows drawn this pass (empty when the page is not on
/// screen). Call after the frame's drawing, in the same pass.
pub fn rows_now(ctx: &Context) -> Vec<DrawnRow> {
    let (pass, rows) = settings_help::drawn_rows(ctx);
    if pass == ctx.cumulative_pass_nr() { rows } else { Vec::new() }
}

impl Md3eUi {
    /// Opens Settings at row `id`, scrolled to it, as a link does, in
    /// «Подробно» (`deep`) or «Просто» (a row «Просто» does not show opens
    /// that visit in «Подробно», as a link does).
    pub fn sweep_open_setting(&mut self, id: SettingId, deep: bool) {
        self.pages.set_settings_deep(deep);
        self.pages.open_setting(id);
    }

    /// Back to plain pages: Now Playing shut at once, no pane, the side
    /// slot and the typed fields of Settings read from the prefs again.
    pub fn sweep_reset(&mut self) {
        self.now_playing = false;
        self.np_anim.snap(0.0);
        self.pane = None;
        self.chrome.np_loaded = false;
        self.pages.forget_typed();
    }

    /// A page of the shell (Now Playing shut at once), with a part of the
    /// side pane open or none.
    pub fn sweep_page(&mut self, ctx: &Context, route: Route, pane: Option<PaneTab>, out: &mut Vec<Command>) {
        self.now_playing = false;
        self.np_anim.snap(0.0);
        if let Route::Library(tab) = route {
            self.pages.library_tab = tab;
        }
        self.navigate(ctx, route, out);
        self.pane = pane;
    }

    /// Now Playing with `slot` beside the cover, opening as from the
    /// player bar (`animate`) or there at once. The slot is not saved.
    pub fn sweep_now_playing(&mut self, ctx: &Context, slot: NpSlot, animate: bool) {
        self.pane = None;
        self.set_now_playing(ctx, true);
        if !animate {
            self.np_anim.snap(1.0);
        }
        self.chrome.np_loaded = true;
        self.chrome.np_slot = slot;
        self.chrome.np_saved = Some(slot);
    }
}

/// The settings page drawn without a window, for what it shows: each
/// row's label and group, and which rows a value shows, hides or fades.
pub struct Survey {
    ctx: Context,
    ui: Md3eUi,
    state: ViewState,
    time: f64,
    size: egui::Vec2,
}

impl Survey {
    /// `state` stands for the runtime's (its accounts and download sources
    /// decide some rows); `size` is the window's.
    pub fn new(state: ViewState, size: [f32; 2]) -> Self {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Self { ctx, ui: Md3eUi::new(), state, time: 0.0, size: vec2(size[0], size[1]) }
    }

    /// The rows of `tab` as the page draws them with `prefs`, in
    /// «Подробно».
    pub fn tab(&mut self, prefs: &Prefs, tab: usize) -> Vec<DrawnRow> {
        self.state.prefs = prefs.clone();
        // A first start's prefs would show the first run over the page.
        self.state.prefs.first_run = false;
        self.ui.pages.set_settings_deep(true);
        self.ui.pages.open_setting(tab_first(tab));
        let mut rows = Vec::new();
        for _ in 0..3 {
            self.time += 0.5;
            let mut out = Vec::new();
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)),
                    time: Some(self.time),
                    ..Default::default()
                },
                |root| self.ui.frame(root, &self.state, &mut out),
            );
            output.textures_delta.clear();
            let (pass, drawn) = settings_help::drawn_rows(&self.ctx);
            rows = if pass + 1 >= self.ctx.cumulative_pass_nr() { drawn } else { Vec::new() };
        }
        rows.retain(|r| r.id.tab() == tab);
        rows
    }
}

/// A view state where the rows that wait for the runtime are drawn: every
/// download source available.
pub fn survey_state() -> ViewState {
    let mut state = ViewState::default();
    state.downloads.sources = v::SourceKey::ALL
        .iter()
        .map(|k| v::SourceView { key: *k, name: k.display_name().to_owned(), available: true, status: None })
        .collect();
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The position of `id` in [`ALL`]: no catch-all, so a new row stops
    /// the build here until it is listed.
    fn place(id: SettingId) -> usize {
        use SettingId as S;
        let pos = |id: SettingId| ALL.iter().position(|x| *x == id).unwrap_or(usize::MAX);
        match id {
            S::OutputDevice | S::Gapless | S::Crossfade | S::Normalization | S::Equalizer | S::TrackSpeed | S::KeepTimbre
            | S::Autoplay | S::AutoplayWorld | S::SearchPlay | S::ListShuffle(_) | S::Marks | S::PlayerButton(_) | S::NavRoot | S::UiScale
            | S::AlbumOpens | S::AlbumVersions

            | S::ShuffleRadio | S::RadioNoRepeat | S::MixesDownloadedOnly | S::RadioNewShare | S::NewShareBy(_) | S::MixesWithNew
            | S::RadioExplore | S::RadioUnheard | S::GraphUse | S::SoundNeural | S::SoundDevice | S::MixCardsWave | S::DislikeButton | S::DislikeSkips | S::DislikeSkipEverywhere
            | S::RadioSource | S::LyricsStyle | S::LyricsUnit | S::LyricsSmooth
            | S::LightSpeed | S::WaveJump | S::JumpSpeed | S::LyricsLift | S::LyricsScale | S::LyricsLead | S::LyricsDim | S::LyricsCascade
            | S::LyricsGlow | S::LyricsSoftEdges | S::LyricsSoftDepth | S::LyricsSoftStrength | S::LyricsSoftFade | S::LyricsGap
            | S::LyricsGapWaves | S::LyricsGapHeight | S::LyricsGapSize | S::LyricsCountdown | S::WaveFallback | S::LyricsReturn
            | S::LyricsGenius | S::SaveLyrics | S::LyricsCheck | S::LyricsWordsPlaying | S::LyricsLinesAuto | S::LyricsStreams
            | S::LyricsStreamKeep | S::LyricsRecognize | S::LyricsModels | S::Theme | S::Scheme | S::Contrast | S::CoverShape
            | S::ShapeRange | S::ShapePick | S::CoverSpin | S::CoverDir | S::CoverPulse | S::CoverPauseMorph | S::Ring
            | S::RingStyle | S::RingSpin | S::RingDir | S::RingBeat | S::Ambient | S::AmbientStrength | S::BgShapes | S::BgCount
            | S::BgSize | S::BgOpacity | S::BgSpeed | S::BgReact | S::BgColors | S::BgHue | S::ProgressWave | S::NpVolume
            | S::NpVolumeHold | S::CoverClickPause | S::DblClickShape | S::TapWait | S::NpMotion | S::MixesProfiles
            | S::MixesEverything | S::MixesMoods | S::MixesOnRepeat | S::BackgroundCards | S::ProfileNames
            | S::HiddenMixes | S::AlbumDate | S::LikedAlbumsPlace | S::RailCompact | S::ReducedMotion | S::Tooltips
            | S::PaneLyrics | S::PaneInfo | S::TrackMenu | S::SpotifyEnabled | S::Spotify | S::SpotifyPersonal | S::SpotifyPlayback
            | S::SpotifySoloist | S::ImportSpotify | S::YouTubeEnabled | S::YouTube | S::YouTubeCookies | S::ImportYouTube
            | S::SoundCloudEnabled | S::SoundCloud | S::SoundCloudToken | S::ImportSoundCloud | S::DeezerEnabled | S::PushLikes
            | S::AlbumLike | S::SingleLike | S::PlaylistSync | S::ImportFiles | S::ImportSavedSongs | S::StreamCache
            | S::DownloadSource(_) | S::LikeDownload | S::UpgradeBar | S::UpgradeEvery | S::OpusBitrate | S::UpgradeAuto
            | S::FakeFlac | S::FakeRestore | S::Soulseek | S::SoulseekShare | S::Prowlarr | S::NoSeed | S::SeedLimit | S::SeedDays
            | S::SeedRatio | S::DownloadLimit | S::UploadLimit | S::SoulseekUpload | S::UserName | S::Autostart
            | S::AutostartHidden | S::FpsLimit | S::TrayKeepWindow | S::AutoDownloads | S::StreamAnalysis | S::LyricsSeparate
            | S::LyricsDeep | S::LyricsWordsAll | S::ToastTime | S::DownloadToasts | S::LibraryDir | S::MastersDir | S::Inbox
            | S::Trash | S::ExportData | S::LyricsUi | S::LyricsGpu | S::LyricsModelsAuto | S::MusicBrainz | S::AcoustId
            | S::LastfmKey | S::ListenbrainzToken | S::BackgroundCountsLess | S::ProfilesSplit | S::GraphServices
            | S::GraphOpenData | S::GraphPersonal | S::LibraryAsTaste | S::DislikeInRecs | S::DemoProfile | S::FirstRun | S::CoverSpinSpeed
            | S::CoverPulseStrength | S::RingSpinSpeed | S::RingPause | S::HomeSections | S::Preset(_) => pos(id),
        }
    }

    /// Every [`SettingId`] (each kind of radio, each download source) is
    /// in [`ALL`] once, and its key reads back.
    #[test]
    fn the_list_covers_every_setting_once() {
        let mut every: Vec<SettingId> = ALL.to_vec();
        every.extend(KINDS.iter().map(|k| SettingId::NewShareBy(*k)));
        every.extend(v::ListKind::ALL.iter().map(|k| SettingId::ListShuffle(*k)));
        every.extend(v::PlayerButton::ALL.iter().map(|b| SettingId::PlayerButton(*b)));
        every.extend(v::SourceKey::ALL.iter().map(|k| SettingId::DownloadSource(*k)));
        every.extend(Area::ALL.iter().filter(|a| !a.names().is_empty()).map(|a| SettingId::Preset(*a)));
        for id in every {
            let at = place(id);
            assert!(at < ALL.len(), "{id:?} is not in ALL");
            assert_eq!(ALL.iter().filter(|x| **x == id).count(), 1, "{id:?} twice in ALL");
            assert_eq!(parse(&key(id)), Some(id), "{id:?}");
        }
        for k in KINDS {
            assert!(ALL.contains(&SettingId::NewShareBy(k)));
        }
        for k in v::SourceKey::ALL {
            assert!(ALL.contains(&SettingId::DownloadSource(k)));
        }
    }

    /// Every value of every row changes the prefs, each to something of
    /// its own (from any other value of the row, it is a change), and a
    /// choice row then shows the option set.
    #[test]
    fn every_value_changes_its_pref() {
        let bases = [Prefs::default(), Prefs::first_start()];
        for base in bases {
            for &id in ALL {
                let mut en = base.clone();
                enable(id, &mut en);
                let vals = values(id, &en);
                if !matches!(spec(id), Spec::Fixed(..)) && !matches!(spec(id), Spec::OutputDevice) {
                    assert!(vals.len() >= 2, "{id:?} has fewer than two values");
                }
                let set: Vec<Prefs> = (0..vals.len())
                    .map(|i| {
                        let mut p = en.clone();
                        assert!(apply(id, i, &mut p), "{id:?} value {i} does not apply");
                        p
                    })
                    .collect();
                for (i, a) in set.iter().enumerate() {
                    for (j, b) in set.iter().enumerate() {
                        if i == j {
                            continue;
                        }
                        let mut from_b = b.clone();
                        apply(id, i, &mut from_b);
                        assert_ne!(&from_b, b, "{id:?}: value {} does not change the prefs from {}", vals[i].key, vals[j].key);
                        assert_ne!(a, b, "{id:?}: values {} and {} write the same", vals[i].key, vals[j].key);
                    }
                    assert_eq!(shown(id, a), Some(i), "{id:?}: value {} reads back as another", vals[i].key);
                }
                assert!(!apply(id, vals.len(), &mut en.clone()), "{id:?}: a value past the end applies");
            }
        }
    }

    /// [`ALL`] runs tab by tab, in the tabs' order.
    #[test]
    fn the_list_goes_tab_by_tab() {
        let tabs: Vec<usize> = ALL.iter().map(|id| id.tab()).collect();
        assert!(tabs.windows(2).all(|w| w[0] <= w[1]), "{tabs:?}");
        for tab in 0..TABS.len() {
            assert_eq!(tab_first(tab).tab(), tab);
            assert_eq!(tab_first(tab).level(), Level::Simple);
        }
    }

    #[test]
    fn slider_stops_are_on_their_steps_and_ends() {
        assert_eq!(stops(3.0, 16.0, 1.0), vec![3.0, 6.0, 10.0, 13.0, 16.0]);
        let keys: Vec<String> = stops(-0.5, 0.5, 0.05).into_iter().map(num_key).collect();
        assert_eq!(keys, ["-0.5", "-0.25", "0", "0.25", "0.5"]);
        assert_eq!(num_key(2.000_000_3), "2");
        assert_eq!(num_key(144.0), "144");
    }

    /// The page draws rows on every tab with labels and groups, all of
    /// them listed.
    #[test]
    fn the_survey_finds_the_rows_of_every_tab() {
        let mut s = Survey::new(survey_state(), [1600.0, 1000.0]);
        for tab in 0..TABS.len() {
            let rows = s.tab(&Prefs::default(), tab);
            assert!(rows.len() > 5, "tab {tab}: {} rows", rows.len());
            for r in &rows {
                assert!(ALL.contains(&r.id), "{:?}", r.id);
                assert!(!r.label.is_empty() && !r.group.is_empty(), "{:?}", r.id);
            }
        }
    }
}
