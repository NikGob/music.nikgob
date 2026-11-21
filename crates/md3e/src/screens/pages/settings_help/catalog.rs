
use nikgob_core::i18n::Locale;
use nikgob_view::{self as v, Prefs};
use nikgob_core::i18n::settings::{
    settings_choices_in, settings_misc, settings_row_help_in,
    settings_row_labels_in,
};

use crate::screens::now_playing;

use super::*;

use super::preview::Preview;

/// Where a radio of `kind` takes its tracks, in a few words (the line under
/// the queue's radio header, which opens the choices).
pub fn radio_source_line_of(p: &Prefs, kind: v::RadioKind) -> &'static str {
    let m = settings_misc();
    match radio_source_of(p, kind) {
        0 => m.radio_source_downloaded,
        1 => m.radio_source_lib,
        _ => m.radio_source_lib_new,
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

macro_rules! c {
    ($label:expr, $help:expr) => {
        Choice { label: $label, help: $help }
    };
}

const fn lang_choice_help(pref: nikgob_core::i18n::LangPref, loc: nikgob_core::i18n::Locale) -> &'static str {
    use nikgob_core::i18n::{LangPref, settings_in};
    let s = settings_in(loc);
    match pref {
        LangPref::Auto => s.lang_auto_help,
        LangPref::Ru => s.lang_ru_help,
        LangPref::En => s.lang_en_help,
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
            DownloadSource(_) | LikeDownload | UpgradeBar | UpgradeEvery | OpusBitrate | UpgradeAuto | YoutubeFallback | FakeFlac | FakeRestore
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
            | ImportFiles | ImportSavedSongs | StreamCache | OpusBitrate | UpgradeAuto | YoutubeFallback | FakeFlac | FakeRestore
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
    pub fn label_in(self, loc: Locale) -> &'static str {
        use SettingId::*;
        let l = settings_row_labels_in(loc);
        match self {
            Language => nikgob_core::i18n::settings_in(loc).language_label,
            Theme => l.theme,
            Contrast => l.contrast,
            Scheme => l.scheme,
            ProgressWave => l.progress_wave,
            NpVolume => l.np_volume,
            NpVolumeHold => l.np_volume_hold,
            CoverShape => l.cover_shape,
            ShapeRange => l.shape_range,
            ShapePick => l.shape_pick,
            CoverPauseMorph => l.cover_pause_morph,
            CoverSpin => l.cover_spin,
            CoverDir => l.cover_dir,
            CoverPulse => l.cover_pulse,
            CoverSpinSpeed => l.cover_spin_speed,
            CoverPulseStrength => l.cover_pulse_strength,
            HomeSections => l.home_sections,
            Preset(a) => preset_title_in(loc, a),
            Ring => l.ring,
            RingStyle => l.ring_style,
            Ambient => l.ambient,
            AmbientStrength => l.ambient_strength,
            BgShapes => l.bg_shapes,
            BgCount => l.bg_count,
            BgSize | LyricsGapSize => l.size,
            BgOpacity => l.bg_opacity,
            BgSpeed => l.speed,
            BgReact => l.bg_react,
            BgColors => l.bg_colors,
            BgHue => l.bg_hue,
            LyricsStyle => l.lyrics_style,
            WaveFallback => l.wave_fallback,
            WaveJump => l.wave_jump,
            JumpSpeed => l.jump_speed,
            LightSpeed => l.light_speed,
            LyricsSmooth => l.lyrics_smooth,
            LyricsDim => l.lyrics_dim,
            LyricsScale => l.lyrics_scale,
            LyricsLead => l.lyrics_lead,
            LyricsCascade => l.lyrics_cascade,
            LyricsGlow => l.lyrics_glow,
            LyricsLift => l.lyrics_lift,
            LyricsGap => l.lyrics_gap,
            LyricsGapWaves => l.lyrics_gap_waves,
            LyricsGapHeight => l.lyrics_gap_height,
            LyricsCountdown => l.lyrics_countdown,
            LyricsUnit => l.lyrics_unit,
            LyricsSoftEdges => l.lyrics_soft_edges,
            LyricsSoftStrength => l.lyrics_soft_strength,
            LyricsSoftDepth => l.lyrics_soft_depth,
            LyricsSoftFade => l.lyrics_soft_fade,
            UiScale => l.ui_scale,
            RailCompact => l.rail_compact,
            AlbumDate => l.album_date,
            PaneLyrics => l.pane_lyrics,
            PaneInfo => l.pane_info,
            TrackMenu => l.track_menu,
            PlayerButton(v::PlayerButton::Sound) => l.speed_and_eq,
            PlayerButton(v::PlayerButton::Shuffle) => l.button_shuffle,
            PlayerButton(v::PlayerButton::Repeat) => l.button_repeat,
            PlayerButton(v::PlayerButton::Like) => l.button_like,
            PlayerButton(v::PlayerButton::AddToPlaylist) => l.button_add_to_playlist,
            PlayerButton(v::PlayerButton::Queue) => l.button_queue,
            PlayerButton(v::PlayerButton::Lyrics) => l.pane_lyrics,
            NavRoot => l.nav_root,
            NpMotion => l.np_motion,
            ReducedMotion => l.reduced_motion,
            Tooltips => l.tooltips,
            FpsLimit => l.fps_limit,
            Autostart => if cfg!(windows) { l.autostart_windows } else { l.autostart_system },
            AutostartHidden => l.autostart_hidden,
            TrayKeepWindow => l.tray_keep_window,
            CoverClickPause => l.cover_click_pause,
            DblClickShape => l.dbl_click_shape,
            TapWait => l.tap_wait,
            ToastTime => l.toast_time,
            SaveLyrics => l.save_lyrics,
            LyricsReturn => l.lyrics_return,
            LyricsCheck => l.lyrics_check,
            LyricsKeepMisfits => l.lyrics_keep_misfits,
            LyricsLinesAuto => l.lyrics_lines_auto,
            LyricsUi => l.lyrics_ui,
            LyricsWordsPlaying => l.lyrics_words_playing,
            LyricsWordsAll => l.lyrics_words_all,
            LyricsRecognize => l.lyrics_recognize,
            LyricsSeparate => l.lyrics_separate,
            LyricsDeep => l.lyrics_deep,
            LyricsGpu => l.lyrics_gpu,
            LyricsModelsAuto => l.lyrics_models_auto,
            LyricsGenius => l.lyrics_genius,
            LyricsStreams => l.lyrics_streams,
            LyricsStreamKeep => l.lyrics_stream_keep,
            LyricsModels => l.lyrics_models,
            ListShuffle(v::ListKind::Mix) => l.list_shuffle_mix,
            ListShuffle(v::ListKind::OwnPlaylist) => l.list_shuffle_own,
            ListShuffle(v::ListKind::Album) => l.list_shuffle_albums,
            ListShuffle(v::ListKind::ReadyPlaylist) => l.list_shuffle_ready,
            ShuffleRadio => l.shuffle_radio,
            OutputDevice => l.output_device,
            Gapless => l.gapless,
            Crossfade => l.crossfade,
            Normalization => l.normalization,
            Equalizer => l.equalizer,
            TrackSpeed => l.track_speed,
            KeepTimbre => l.keep_timbre,
            LibraryDir => l.library_dir,
            MastersDir => l.masters_dir,
            Inbox => "Inbox",
            Trash => l.trash,
            ExportData => l.export_data,
            OpusBitrate => l.opus_bitrate,
            FakeFlac => l.fake_flac,
            FakeRestore => l.fake_restore,
            MusicBrainz => l.musicbrainz,
            AcoustId => l.acoustid,
            DemoProfile => l.demo_profile,
            FirstRun => l.first_run,
            UserName => l.user_name,
            RadioNewShare => l.radio_new_share,
            MixesDownloadedOnly => l.mixes_downloaded_only,
            Autoplay | NewShareBy(v::RadioKind::Autoplay) => l.autoplay,
            AutoplayWorld => l.autoplay_world,
            SearchPlay => l.search_play,
            RadioSource => l.radio_source,
            RadioNoRepeat => l.radio_no_repeat,
            RadioExplore => l.radio_explore,
            RadioUnheard => l.radio_unheard,
            SoundNeural => l.sound_neural,
            SoundDevice => l.sound_device,
            MixCardsWave => l.mix_cards_wave,
            NewShareBy(v::RadioKind::Track) => l.radio_track,
            NewShareBy(v::RadioKind::Artist) => l.radio_artist,
            NewShareBy(v::RadioKind::Mix) => l.radio_mix,
            MixesWithNew => l.mixes_with_new,
            BackgroundCountsLess => l.background_counts_less,
            ProfilesSplit => l.profiles_split,
            ProfileNames => l.profile_names,
            MixesProfiles => l.mixes_profiles,
            MixesEverything => l.mixes_everything,
            MixesMoods => l.mixes_moods,
            MixesOnRepeat => l.mixes_on_repeat,
            BackgroundCards => l.background_cards,
            DislikeButton => l.dislike_button,
            DislikeSkips => l.dislike_skips,
            DislikeSkipEverywhere => l.dislike_skip_everywhere,
            DislikeInRecs => l.dislike_in_recs,
            Marks => l.marks,
            AlbumOpens => l.album_opens,
            AlbumVersions => l.album_versions,
            HiddenMixes => l.hidden_mixes,
            GraphUse => l.graph_use,
            GraphServices => l.graph_services,
            GraphOpenData => l.graph_open_data,
            GraphPersonal => l.graph_personal,
            LibraryAsTaste => l.library_as_taste,
            LastfmKey => l.lastfm_key,
            ListenbrainzToken => l.listenbrainz_token,
            SpotifyEnabled | YouTubeEnabled | DeezerEnabled | SoundCloudEnabled => l.use_switch,
            Spotify | YouTube | SoundCloud => l.account,
            SpotifyPlayback => l.spotify_playback,
            SpotifySoloist => "Spotify Soloist",
            SpotifyPersonal => l.spotify_personal,
            YouTubeCookies => l.youtube_cookies,
            SoundCloudToken => l.soundcloud_token,
            ImportSpotify | ImportYouTube | ImportSoundCloud => l.playlists_and_history,
            LikeDownload => l.like_download,
            PushLikes => l.push_likes,
            AlbumLike => l.album_like,
            SingleLike => l.single_like,
            LikedAlbumsPlace => l.liked_albums_place,
            AutoDownloads => l.auto_downloads,
            DownloadToasts => l.download_toasts,
            UpgradeEvery => l.upgrade_every,
            UpgradeBar => l.upgrade_bar,
            UpgradeAuto => l.upgrade_auto,
            YoutubeFallback => l.youtube_fallback,
            DownloadSource(k) => k.display_name(),
            PlaylistSync => l.playlist_sync,
            ImportFiles => l.import_files,
            ImportSavedSongs => l.import_saved_songs,
            StreamCache => l.stream_cache,
            StreamAnalysis => l.stream_analysis,
            Prowlarr => "Prowlarr",
            Soulseek => "Soulseek",
            SoulseekShare => l.soulseek_share,
            SoulseekUpload => l.soulseek_upload,
            NoSeed => l.no_seed,
            SeedLimit => l.seed_limit,
            SeedDays => l.seed_days,
            SeedRatio => l.seed_ratio,
            DownloadLimit => l.download_limit,
            UploadLimit => l.upload_limit,
            ApiEnabled => l.use_switch,
            ApiAccess => l.api_access,
            ApiPort => l.api_port,
            ApiTokens => l.api_tokens,
            MeshEnabled => l.mesh_enabled,
            MeshDevices => l.mesh_devices,
            MeshNearby => l.mesh_nearby,
            MeshKeepMasters => l.mesh_keep_masters,
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
    pub fn help_in(self, loc: Locale) -> &'static str {
        use SettingId::*;
        let h = settings_row_help_in(loc);
        match self {
            Language => nikgob_core::i18n::settings_in(loc).language_help,
            Theme => h.theme,
            Contrast => h.contrast,
            Scheme => h.scheme,
            ProgressWave => h.progress_wave,
            NpVolume => h.np_volume,
            NpVolumeHold => h.np_volume_hold,
            CoverShape => h.cover_shape,
            ShapeRange => h.shape_range,
            ShapePick => h.shape_pick,
            CoverPauseMorph => h.cover_pause_morph,
            CoverSpin => h.cover_spin,
            CoverDir => h.cover_dir,
            CoverPulse => h.cover_pulse,
            CoverSpinSpeed => h.cover_spin_speed,
            CoverPulseStrength => h.cover_pulse_strength,
            HomeSections => h.home_sections,
            Preset(v::presets::Area::Recs) => h.preset_recs,
            Preset(v::presets::Area::Lyrics) => h.preset_lyrics_view,
            Preset(v::presets::Area::Track) => h.preset_track,
            Preset(v::presets::Area::Sources) => h.preset_sources,
            Preset(v::presets::Area::Seeding) => h.preset_seeding,
            Preset(v::presets::Area::Load) => h.preset_load,
            Ring => h.ring,
            RingStyle => h.ring_style,
            Ambient => h.ambient,
            AmbientStrength => h.ambient_strength,
            BgShapes => h.bg_shapes,
            BgCount => h.bg_count,
            BgSize => h.bg_size,
            BgOpacity => h.bg_opacity,
            BgSpeed => h.bg_speed,
            BgReact => h.bg_react,
            BgColors => h.bg_colors,
            BgHue => h.bg_hue,
            LyricsStyle => h.lyrics_style,
            WaveFallback => h.wave_fallback,
            WaveJump => h.wave_jump,
            JumpSpeed => h.jump_speed,
            LightSpeed => h.light_speed,
            LyricsSmooth => h.lyrics_smooth,
            LyricsDim => h.lyrics_dim,
            LyricsScale => h.lyrics_scale,
            LyricsLead => h.lyrics_lead,
            LyricsCascade => h.lyrics_cascade,
            LyricsGlow => h.lyrics_glow,
            LyricsLift => h.lyrics_lift,
            LyricsGap => h.lyrics_gap,
            LyricsGapWaves => h.lyrics_gap_waves,
            LyricsGapHeight => h.lyrics_gap_height,
            LyricsGapSize => h.lyrics_gap_size,
            LyricsCountdown => h.lyrics_countdown,
            LyricsUnit => h.lyrics_unit,
            LyricsSoftEdges => h.lyrics_soft_edges,
            LyricsSoftStrength => h.lyrics_soft_strength,
            LyricsSoftDepth => h.lyrics_soft_depth,
            LyricsSoftFade => h.lyrics_soft_fade,
            UiScale => h.ui_scale,
            RailCompact => h.rail_compact,
            AlbumDate => h.album_date,
            PaneLyrics => h.pane_lyrics,
            PaneInfo => h.pane_info,
            TrackMenu => h.track_menu,
            PlayerButton(v::PlayerButton::Sound) => h.player_speed_eq,
            PlayerButton(v::PlayerButton::Shuffle) => h.button_shuffle,
            PlayerButton(v::PlayerButton::Repeat) => h.button_repeat,
            PlayerButton(v::PlayerButton::Like) => h.button_like,
            PlayerButton(v::PlayerButton::AddToPlaylist) => h.button_playlist,
            PlayerButton(v::PlayerButton::Queue) => h.button_queue,
            PlayerButton(v::PlayerButton::Lyrics) => h.button_lyrics,
            NavRoot => h.nav_root,
            NpMotion => h.np_motion,
            ReducedMotion => h.reduced_motion,
            Tooltips => h.tooltips,
            FpsLimit => h.fps_limit,
            Autostart => if cfg!(windows) { h.autostart_windows } else { h.autostart_system },
            AutostartHidden => if cfg!(windows) { h.autostart_hidden_windows } else { h.autostart_hidden_system },
            TrayKeepWindow => h.tray_keep_window,
            CoverClickPause => h.cover_click_pause,
            DblClickShape => h.dbl_click_shape,
            TapWait => h.tap_wait,
            ToastTime => h.toast_time,
            SaveLyrics => h.save_lyrics,
            LyricsReturn => h.lyrics_return,
            LyricsCheck => h.lyrics_check,
            LyricsKeepMisfits => h.lyrics_keep_misfits,
            LyricsUi => h.lyrics_ui,
            LyricsLinesAuto => h.lyrics_lines_auto,
            LyricsWordsPlaying => h.lyrics_words_playing,
            LyricsWordsAll => h.lyrics_words_all,
            LyricsRecognize => h.lyrics_recognize,
            LyricsSeparate => h.lyrics_separate,
            LyricsDeep => h.lyrics_deep,
            LyricsGpu => h.lyrics_gpu,
            LyricsModelsAuto => h.lyrics_models_auto,
            LyricsGenius => h.lyrics_genius,
            LyricsStreams => h.lyrics_streams,
            LyricsStreamKeep => h.lyrics_stream_keep,
            LyricsModels => h.lyrics_models,
            OutputDevice => h.output_device,
            ListShuffle(v::ListKind::Mix) => h.list_shuffle_mix,
            ListShuffle(v::ListKind::OwnPlaylist) => h.list_shuffle_own,
            ListShuffle(v::ListKind::Album) => h.list_shuffle_albums,
            ListShuffle(v::ListKind::ReadyPlaylist) => h.list_shuffle_ready,
            ShuffleRadio => h.shuffle_radio,
            Gapless => h.gapless,
            Crossfade => h.crossfade,
            Normalization => h.normalization,
            Equalizer => h.equalizer,
            TrackSpeed => h.track_speed,
            KeepTimbre => h.keep_timbre,
            LibraryDir => h.library_dir,
            MastersDir => h.masters_dir,
            Inbox => h.inbox,
            Trash => h.trash,
            ExportData => h.export_data,
            OpusBitrate => h.opus_bitrate,
            FakeFlac => h.fake_flac,
            FakeRestore => h.fake_restore,
            MusicBrainz => h.musicbrainz,
            AcoustId => h.acoustid,
            DemoProfile => h.demo_profile,
            FirstRun => h.first_run,
            UserName => h.user_name,
            Prowlarr => h.prowlarr,
            Soulseek => h.soulseek,
            SoulseekShare => h.soulseek_share,
            SoulseekUpload => h.soulseek_upload,
            NoSeed => h.no_seed,
            SeedLimit => h.seed_limit,
            SeedDays => h.seed_days,
            SeedRatio => h.seed_ratio,
            DownloadLimit => h.download_limit,
            UploadLimit => h.upload_limit,
            ApiEnabled => h.api_enabled,
            ApiAccess => h.api_access,
            ApiPort => h.api_port,
            ApiTokens => h.api_tokens,
            MeshEnabled => h.mesh_enabled,
            MeshDevices => h.mesh_devices,
            MeshKeepMasters => h.mesh_keep_masters,
            MeshNearby => h.mesh_nearby,
            Spotify => h.spotify,
            SpotifyEnabled => h.spotify_enabled,
            SpotifyPlayback => h.spotify_playback,
            SpotifySoloist => h.spotify_soloist,
            SpotifyPersonal => h.spotify_personal,
            YouTube => h.youtube,
            YouTubeEnabled => h.youtube_enabled,
            DeezerEnabled => h.deezer_enabled,
            SoundCloud => h.soundcloud,
            SoundCloudEnabled => h.soundcloud_enabled,
            SoundCloudToken => h.soundcloud_token,
            ImportSpotify => h.import_spotify,
            ImportYouTube => h.import_youtube,
            ImportSoundCloud => h.import_soundcloud,
            YouTubeCookies => h.youtube_cookies,
            LikeDownload => h.like_download,
            PushLikes => h.push_likes,
            AlbumLike => h.album_like,
            SingleLike => h.single_like,
            LikedAlbumsPlace => h.liked_albums_place,
            AutoDownloads => h.auto_downloads,
            DownloadToasts => h.download_toasts,
            UpgradeEvery => h.upgrade_every,
            UpgradeBar => h.upgrade_bar,
            UpgradeAuto => h.upgrade_auto,
            YoutubeFallback => h.youtube_fallback,
            DownloadSource(v::SourceKey::Torrents) => h.download_torrents,
            DownloadSource(v::SourceKey::Khinsider) => h.download_khinsider,
            DownloadSource(v::SourceKey::Soulseek) => h.download_soulseek,
            DownloadSource(v::SourceKey::Qobuz) => h.download_qobuz,
            DownloadSource(v::SourceKey::Tidal) => h.download_tidal,
            DownloadSource(v::SourceKey::Squidify) => h.download_squidify,
            DownloadSource(v::SourceKey::Archive) => h.download_archive,
            DownloadSource(v::SourceKey::Bandcamp) => h.download_bandcamp,
            DownloadSource(v::SourceKey::Soundcloud) => h.download_soundcloud,
            DownloadSource(v::SourceKey::Dogmazic) => h.download_dogmazic,
            DownloadSource(v::SourceKey::Junkyard) => h.download_junkyard,
            PlaylistSync => h.playlist_sync,
            ImportFiles => h.import_files,
            ImportSavedSongs => h.import_saved_songs,
            StreamCache => h.stream_cache,
            StreamAnalysis => h.stream_analysis,
            RadioNewShare => h.radio_new_share,
            Autoplay => h.autoplay,
            SearchPlay => h.search_play,
            AutoplayWorld => h.autoplay_world,
            RadioSource => h.radio_source,
            MixesDownloadedOnly => h.mixes_downloaded_only,
            SoundDevice => h.sound_device,
            MixCardsWave => h.mix_cards_wave,
            NewShareBy(v::RadioKind::Track) => h.radio_track,
            NewShareBy(v::RadioKind::Artist) => h.radio_artist,
            NewShareBy(v::RadioKind::Mix) => h.radio_mix,
            NewShareBy(v::RadioKind::Autoplay) => h.new_by_autoplay,
            MixesWithNew => h.mixes_with_new,
            BackgroundCountsLess => h.background_counts_less,
            ProfilesSplit => h.profiles_split,
            ProfileNames => h.profile_names,
            MixesProfiles => h.mixes_profiles,
            MixesEverything => h.mixes_everything,
            MixesMoods => h.mixes_moods,
            MixesOnRepeat => h.mixes_on_repeat,
            RadioNoRepeat => h.radio_no_repeat,
            RadioExplore => h.radio_explore,
            RadioUnheard => h.radio_unheard,
            BackgroundCards => h.background_cards,
            DislikeButton => h.dislike_button,
            DislikeSkips => h.dislike_skips,
            DislikeSkipEverywhere => h.dislike_skip_everywhere,
            DislikeInRecs => h.dislike_in_recs,
            Marks => h.marks,
            AlbumOpens => h.album_opens,
            AlbumVersions => h.album_versions,
            HiddenMixes => h.hidden_mixes,
            SoundNeural => h.sound_neural,
            GraphUse => h.graph_use,
            GraphServices => h.graph_services,
            GraphOpenData => h.graph_open_data,
            GraphPersonal => h.graph_personal,
            LibraryAsTaste => h.library_as_taste,
            LastfmKey => h.lastfm_key,
            ListenbrainzToken => h.listenbrainz_token,
        }
    }

    /// One line on what the option changes in the active process locale.
    pub fn help(self) -> &'static str {
        self.help_in(nikgob_core::i18n::locale())
    }

    /// The options of a choice row (segmented buttons or a menu) in `loc`, in
    /// order; empty for every other row.
    pub fn choices_in(self, loc: Locale) -> &'static [Choice] {
        use v::presets::Area;
        let c = settings_choices_in(loc);
        match self {
            Self::Language => match loc {
                Locale::Ru => &LANG_CHOICES_RU,
                Locale::En => &LANG_CHOICES_EN,
            },
            Self::Preset(Area::Recs) => &c.preset_recs,
            Self::Preset(Area::Lyrics) => &c.preset_lyrics,
            Self::Preset(Area::Track) => &c.preset_track,
            Self::Preset(Area::Sources) => &c.preset_sources,
            Self::Preset(Area::Seeding) => &c.preset_seeding,
            Self::Preset(Area::Load) => &c.preset_load,
            Self::Theme => &c.theme,
            Self::Contrast => &c.contrast,
            Self::Scheme => &c.scheme,
            Self::NpVolume => &c.np_volume,
            Self::CoverShape => &c.cover_shape,
            Self::LyricsUi => &c.lyrics_ui,
            Self::LyricsStreamKeep => &c.lyrics_stream_keep,
            Self::LyricsGpu => &c.lyrics_gpu,
            Self::CoverDir => &c.cover_dir,
            Self::AlbumDate => &c.album_date,
            Self::RingStyle => &c.ring_style,
            Self::Ambient => &c.ambient,
            Self::BgShapes => &c.bg_shapes,
            Self::BgColors => &c.bg_colors,
            Self::BgHue => &c.bg_hue,
            Self::LyricsStyle => &c.lyrics_style,
            Self::LyricsUnit => &c.lyrics_unit,
            Self::WaveFallback => &c.wave_fallback,
            Self::LyricsGap => &c.lyrics_gap,
            Self::NpMotion => &c.np_motion,
            Self::ToastTime => &c.toast_time,
            Self::DblClickShape => &c.dbl_click_shape,
            Self::LikeDownload => &c.like_download,
            Self::AlbumLike => &c.album_like,
            Self::SingleLike => &c.single_like,
            Self::AlbumOpens => &c.album_opens,
            Self::LikedAlbumsPlace => &c.liked_albums_place,
            Self::UpgradeEvery => &c.upgrade_every,
            Self::UpgradeBar => &c.upgrade_bar,
            Self::UpgradeAuto => &c.upgrade_auto,
            Self::YoutubeFallback => &c.youtube_fallback,
            Self::PlaylistSync => &c.playlist_sync,
            Self::RadioSource => &c.radio_source,
            Self::RadioNewShare | Self::NewShareBy(_) => &c.radio_new_share,
            Self::BackgroundCards => &c.background_cards,
            Self::RadioExplore => &c.radio_explore,
            Self::SoundNeural => &c.sound_neural,
            Self::MixCardsWave => &c.mix_cards,
            Self::SoundDevice => &c.sound_device,
            Self::SearchPlay => &c.search_play,
            Self::RadioUnheard => &c.radio_unheard,
            Self::RadioNoRepeat => &c.radio_no_repeat,
            Self::ProfileNames => &c.profile_names,
            Self::FakeFlac => &c.fake_flac,
            Self::ListShuffle(_) => &c.list_shuffle,
            Self::OpusBitrate => &c.opus_bitrate,
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
            Self::YoutubeFallback => usize::from(p.youtube_fallback),
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
            Self::MixCardsWave => super::super::cards::MixClick::ALL.iter().position(|c| *c == super::super::cards::MixClick::chosen(p)).unwrap_or(0),
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
            Self::YoutubeFallback => p.youtube_fallback = i == 1,
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
            Self::MixCardsWave => super::super::cards::MixClick::ALL[i.min(2)].choose(p),
            Self::ListShuffle(kind) => p.list_shuffle.set(kind, v::ListShuffle::ALL[i.min(v::ListShuffle::ALL.len() - 1)]),
            _ => {}
        }
    }

    pub(super) fn preview(self) -> Option<Preview> {
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
pub fn preset_title_in(loc: Locale, a: v::presets::Area) -> &'static str {
    use v::presets::Area;
    let l = settings_row_labels_in(loc);
    match a {
        Area::Recs => l.preset_recs,
        Area::Lyrics => l.preset_lyrics_view,
        Area::Track => l.preset_track,
        Area::Sources => l.preset_sources,
        Area::Seeding => l.preset_seeding,
        Area::Load => l.preset_load,
    }
}

/// The title of the group an area's presets head, in the active locale.
#[cfg(test)]
pub fn preset_title(a: v::presets::Area) -> &'static str {
    preset_title_in(nikgob_core::i18n::locale(), a)
}

/// The index of `m` in [`v::PlaylistSyncMode::ALL`] (the order of the
/// "Синхронизация" choices).
pub fn sync_index(m: v::PlaylistSyncMode) -> usize {
    v::PlaylistSyncMode::ALL.iter().position(|x| *x == m).unwrap_or(1)
}
