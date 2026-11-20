//! The preferences: their schema, the defaults, the patches and the
//! upgrades.

use std::path::PathBuf;

use super::*;

/// The revision of the lyrics defaults: 1 turned word timing off for the
/// playing song and for the library, recognition from scratch off, and
/// line timing for plain texts on; 2 made «Окно» the look of the lyrics
/// control; 3 turned word timing for the playing song on and moved the
/// lead from [`OLD_LYRICS_LEAD`] to [`LYRICS_LEAD`]; 4 made «По слогам»
/// the unit of the lyrics' animations ([`Prefs::lyrics_unit`]); 5 hid the
/// player's lyrics button (the key L stays); 6 shows it again by default.
pub const LYRICS_REV: u8 = 6;

/// The defaults of «Перемешивание» the prefs follow ([`Prefs::shuffle_rev`]).
pub const SHUFFLE_REV: u8 = 1;

/// The default of «Альбом показывает» the prefs follow
/// ([`Prefs::album_rev`]): before 1 it was «Автоматически».
pub const ALBUM_REV: u8 = 1;

/// [`Prefs::lyrics_unit`]: letter by letter («По буквам») …
pub const LYRICS_BY_LETTERS: u8 = 0;
/// … syllable by syllable («По слогам», the default) …
pub const LYRICS_BY_SYLLABLES: u8 = 1;
/// … or whole written words («По словам»).
pub const LYRICS_BY_WORDS: u8 = 2;

/// Seconds the lyrics highlight runs ahead of the voice by default
/// ([`Prefs::lyrics_lead`]): about what karaoke apps use, which makes up
/// for the frame being shown a little after it is drawn and for a word's
/// light easing in.
pub const LYRICS_LEAD: f32 = 0.2;
/// The default lead before [`LYRICS_REV`] 3.
pub const OLD_LYRICS_LEAD: f32 = 0.1;

/// User-facing settings the UI edits (the runtime persists them).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub language: LangPref,
    pub design: Design,
    pub theme: ThemeMode,
    pub contrast: u8,
    /// Index into the design system's scheme variants (MD3E: Content first,
    /// TonalSpot «Спокойная» by default).
    pub scheme_variant: u8,
    pub ambient: Ambient,
    pub fps_limit: u32,
    pub reduced_motion: bool,
    /// Hints over buttons on hover (the settings' preview cards show anyway).
    pub tooltips: bool,
    pub gapless: bool,
    pub crossfade_s: f32,
    pub normalization: bool,
    /// «Эквалайзер»: on, and the gain of each of its ten bands in dB
    /// ([`EQ_FREQS`]). One for the player, like the normalization.
    pub eq_on: bool,
    pub eq_gains: [f32; EQ_BANDS],
    /// «Скорость трека»: the «Скорость» item of a track's menu and the
    /// versions chosen with it. Off, every track plays as recorded; the
    /// choices are kept and come back with the switch.
    pub speed_enabled: bool,
    /// «Сохранять тембр голоса»: a track played faster or slower keeps the
    /// character of its voices (the pitch still moves with the speed).
    pub keep_timbre: bool,
    /// «Перемешивание»: how a new list of each kind starts — shuffled, in
    /// its order, or as the shuffle button is ([`ListShuffles`]).
    /// «Перемешать» shuffles anyway; other lists follow the button.
    pub list_shuffle: ListShuffles,
    /// Which defaults of [`Self::list_shuffle`] the prefs were written with
    /// ([`SHUFFLE_REV`]): before 1 own playlists started shuffled by
    /// default, and that default turns into «По кнопке» once.
    pub shuffle_rev: u8,
    /// «Перемешивать радио и автовоспроизведение»: with shuffle on, the
    /// picks of a radio, autoplay and the daylist play shuffled too (off:
    /// they keep their own order).
    pub shuffle_radio: bool,
    pub opus_bitrate: u32,
    pub check_fake_lossless: bool,
    /// A "lossless" file plainly made from a lossy one is offered in
    /// «Подтверди» as an MP3 that keeps all its content really has (tags and
    /// all; the old file goes to the Trash, «Вернуть FLAC» brings it back).
    /// Off: it stays as it is, labelled for what it is, and is not offered.
    pub convert_fake_lossless: bool,
    /// …and becomes the MP3 without asking. Off unless the user says.
    pub fake_flac_auto: bool,
    pub library_dir: PathBuf,
    pub inbox_dir: PathBuf,
    /// Where lossless masters (`.flac` originals and `.json` sidecars) are kept.
    /// Defaults to `<library_dir>/.musiclib/masters`, or another drive/folder chosen by the user.
    pub masters_dir: PathBuf,
    pub fallback_seed: u32,
    pub prowlarr_url: String,
    pub prowlarr_configured: bool,
    pub seed_limit_gb: u32,
    pub seed_days: u32,
    pub no_seed: bool,
    pub output_devices: Vec<String>,
    pub output_device: Option<String>,
    /// MusicBrainz contact (User-Agent); the AcoustID key lives in settings.toml.
    pub metadata_contact: String,
    pub acoustid_configured: bool,
    /// Stop seeding a torrent at this upload/download ratio (0 = no limit).
    pub seed_ratio: f32,
    /// KiB/s, 0 = unlimited.
    pub download_limit_kbps: u32,
    pub upload_limit_kbps: u32,
    /// Content beside the cover in Now Playing.
    pub np_slot: NpSlot,
    /// How synced lyrics animate: 0 karaoke, 1 focus, 2 wave. Prefs saved
    /// with 3 (the old «По словам» animation) move to the wave by words
    /// ([`Prefs::upgrade_lyrics`]).
    pub lyrics_style: u8,
    /// A text with line times only (no word times) uses the calm Focus
    /// under every animation and unit instead of guessing the words' pace.
    pub wave_syllables_only: bool,
    /// How high wave letters (or words) jump as they are sung (0: they only light up).
    pub lyrics_wave_jump: f32,
    /// «Скорость прыжка»: how fast a wave letter (or word) jumps and
    /// settles (0.5..2, 1: Material's expressive fast spatial spring).
    pub lyrics_jump_speed: f32,
    /// «Скорость подсветки»: how fast a letter or word lights up, 0
    /// smoothly .. 1 at once (karaoke: how soft the front's edge is).
    pub lyrics_light_speed: f32,
    /// «Плавность»: 0 keeps to the times exactly .. 1 the karaoke front
    /// takes the most even way through them (never before a time, up to
    /// 0.3 s after it) and the wave's lights and jumps spread over
    /// neighbouring letters (averaged over up to 0.4 s).
    pub lyrics_smooth: f32,
    /// How far lines other than the current one fade (0 none .. 1 strong).
    pub lyrics_dim: f32,
    /// Text size multiplier for synced lyrics (0.8..1.4).
    pub lyrics_scale: f32,
    /// Seconds the lyrics highlight runs ahead of the voice (negative:
    /// behind it), in every animation; seeking and stored times keep theirs.
    pub lyrics_lead: f32,
    /// Lines further from the current one follow the scroll more lazily.
    pub lyrics_cascade: bool,
    /// The sung part glows a little.
    pub lyrics_glow: bool,
    /// «Подъём спетого»: under karaoke the sung words float up a little as
    /// the light comes to them (0 none .. 1 about a twentieth of the text
    /// size). Prefs from before it read 0: the look they had.
    pub lyrics_lift: f32,
    /// Seconds after a manual scroll before the view returns to the song.
    pub lyrics_return_s: f32,
    /// The pause indicator between lines: 0 flat thin, 1 wavy thin,
    /// 2 flat thick, 3 wavy thick (the four Material 3 Expressive forms).
    pub lyrics_gap_style: u8,
    /// Crests around the pause ring (wavelength = circumference / this).
    pub lyrics_gap_waves: f32,
    /// «Высота волн» of the pause ring (0.5..2, 1 the Material height).
    pub lyrics_gap_height: f32,
    /// «Размер индикатора»: the pause ring's size (0.8..1.5).
    pub lyrics_gap_size: f32,
    /// The last seconds of a pause count down in digits instead.
    pub lyrics_countdown: bool,
    /// The old «Подсвечивать» switch (whole written words instead of
    /// letters), before [`Self::lyrics_unit`]: read once by
    /// [`Prefs::upgrade_lyrics`].
    pub lyrics_by_words: bool,
    /// «Шаг подсветки»: what every animation goes over, [`LYRICS_BY_LETTERS`],
    /// [`LYRICS_BY_SYLLABLES`] (the timed syllables of a text that has
    /// them, the timed words of one that has words) or [`LYRICS_BY_WORDS`]
    /// (each part of a hyphenated word).
    pub lyrics_unit: u8,
    /// «Мягкие края»: lines near the top and bottom of the lyrics blur and
    /// fade out instead of being cut.
    pub lyrics_soft_edges: bool,
    /// «Сила размытия»: how much a line at the very edge is blurred
    /// (0..1.5, a multiplier).
    pub lyrics_soft_strength: f32,
    /// «Глубина»: how far into the lyrics each soft edge reaches, as a
    /// share of their height (0.08..0.4).
    pub lyrics_soft_depth: f32,
    /// «Затухание»: how early a line thins out in the soft edge (0: only
    /// right at the edge .. 1: from where the band starts).
    pub lyrics_soft_fade: f32,
    /// Embed the selected online lyrics in the track tags.
    pub save_found_lyrics: bool,
    /// «Видеокарта»: the lyrics models run on the GPU (DirectML), else on
    /// the CPU (also when the GPU fails).
    pub lyrics_gpu: bool,
    /// «Проверять текст по записи»: a fetched text is aligned to the
    /// track's own audio before it is trusted.
    pub lyrics_check: bool,
    /// «Подсветка по словам во всех песнях»: the background pass makes
    /// word timing for every song with a file (hours on a big library).
    pub lyrics_words_all: bool,
    /// «Строки простого текста»: the playing song's
    /// plain text gets line timing from the recording when no source has
    /// line timing.
    pub lyrics_lines_auto: bool,
    /// «Подсветка по словам в играющей песне»: the playing song's
    /// texts are shown with word timing made from the recording.
    pub lyrics_words_playing: bool,
    /// «Вид»: what the control under the lyrics opens: 0 its menu
    /// («Меню»), 1 a window over the player («Окно», the default), 2 the
    /// «Текст песни» page («Экран»).
    pub lyrics_ui: u8,
    /// Which defaults of the lyrics settings the prefs were written with
    /// ([`LYRICS_REV`]); older prefs are moved to the new defaults once.
    pub lyrics_rev: u8,
    /// «Распознавать текст, если его нет нигде» (experimental).
    pub lyrics_recognize: bool,
    /// «Отделять голос от музыки» before aligning.
    pub lyrics_separate: bool,
    /// «Глубокое создание субтитров»: timing made with the stronger
    /// separator and aligners, slower.
    pub lyrics_deep: bool,
    /// «Скачивать модели сами» when they are needed.
    pub lyrics_models_auto: bool,
    /// «Искать на Genius».
    pub lyrics_genius: bool,
    /// «Подсветка для песен без файла»: timing may be made for a stream
    /// (the audio fetched for a while, then deleted).
    pub lyrics_streams: bool,
    /// What a stream's made timing keeps in its artist's folder: 0 the
    /// text alone («Только текст»), 1 the song as a file and the text
    /// («Песню и текст»).
    pub lyrics_stream_keep: u8,
    /// «Настройки текста…» was picked from the control's menu once: the
    /// menu stops offering it.
    pub lyrics_settings_seen: bool,
    /// The outline the Now Playing cover takes while playing.
    pub cover_shape: CoverShape,
    /// «Менять форму на паузе»: a pause turns the shaped cover into the
    /// rounded square (off: it keeps its shape, only still).
    pub cover_pause_morph: bool,
    /// The cover's outline turns while playing (the picture stays put).
    pub cover_spin: bool,
    /// The cover swells a little on each beat.
    pub cover_pulse: bool,
    /// «Кольцо»: the dotted ring around a shaped cover while it plays
    /// (never around the square cover, never on a pause). It turns against
    /// the cover in [`RING_SPIN_S`] and jumps with the music, unless motion
    /// is reduced. Prefs saved with its old rows (on a pause, the turn, its
    /// way and speed, in time) still read: those keys are let go.
    pub ring: bool,
    /// What the ring is made of and where it sits.
    pub ring_style: RingStyle,
    /// The visualizer's picture (chosen in the visualizer itself).
    pub viz_concept: VizConcept,
    /// The visualizer takes the whole screen (F11 or its button inside it
    /// switch it); off, it fills the window.
    pub viz_fullscreen: bool,
    /// The cover's outline turns counter-clockwise.
    pub cover_ccw: bool,
    /// Now Playing shows a volume slider (otherwise just the mute button;
    /// the wheel still sets the volume).
    pub np_volume: bool,
    /// Instead of a fixed slider: a volume pill pops up at the bottom
    /// centre of Now Playing only while the volume changes.
    pub np_volume_pop: bool,
    /// How long the volume pill stays after the last change, in seconds.
    pub np_volume_pop_hold: f32,
    /// How Now Playing opens and closes: 0 smooth, 1 lively, 2 fast.
    pub np_motion: u8,
    /// Progress bars turn into a wave while playing (off: always flat).
    pub progress_wave: bool,
    /// The shapes [`CoverShape::Random`] picks from; empty = all of them.
    pub random_shapes: Vec<CoverShape>,
    /// Windows starts the app at sign-in (the app reads the system's own
    /// state at start; this only mirrors and edits it).
    pub autostart: bool,
    /// A start at sign-in stays in the tray, with no window.
    pub autostart_hidden: bool,
    /// «Быстрое открытие из трея»: closing the window only hides it, so it
    /// comes back at once (off: it is destroyed and made again, which saves
    /// memory while it waits in the tray).
    pub tray_keep_window: bool,
    /// A click on the Now Playing cover toggles play / pause.
    pub cover_click_pause: bool,
    /// A double click in Now Playing changes the cover outline: 0 never,
    /// 1 only in the random mode, 2 always (a fixed shape steps on).
    pub dbl_click_shape: u8,
    /// How long a tap on the cover waits for a second one, in seconds;
    /// 0 pauses at once (a second tap then undoes it).
    pub np_tap_wait: f32,
    /// Seconds a snackbar with a timeout stays up (one with an undo twice
    /// as long); 0 = until it is closed.
    pub toast_s: u8,
    /// Material shapes behind Now Playing that follow the music.
    pub bg_shapes: BgShapes,
    /// How many of them (4..=32).
    pub bg_shapes_count: u32,
    /// Size multiplier (0.5..=2).
    pub bg_shapes_size: f32,
    /// Opacity multiplier (0.25..=2).
    pub bg_shapes_opacity: f32,
    /// Motion speed multiplier (0.25..=3).
    pub bg_shapes_speed: f32,
    /// How strongly they react to the music (0..=2).
    pub bg_shapes_react: f32,
    /// Colours: 0 theme containers, 1 accent only, 2 bright accents,
    /// 3 the user's hue ([`Self::bg_shapes_hue`]).
    pub bg_shapes_colors: u8,
    /// The hue for colour mode 3: an index into the UI's swatches (0..8).
    pub bg_shapes_hue: u8,
    /// How strong the ambient clouds are (0.25..=2).
    pub ambient_strength: f32,
    /// «Масштаб интерфейса»: the whole window larger or smaller, on top of
    /// the system's scale ([`UI_SCALE_RANGE`], 1 = 100 %).
    pub ui_scale: f32,
    /// The navigation rail always stays narrow (icons only) and its expand
    /// button is gone.
    pub rail_compact: bool,
    /// «Дата альбома»: how much of an album's release date its page shows.
    pub album_date: AlbumDate,
    /// The parts of the side pane by the content, each on its own: none by
    /// default, and a part that is off has no button, menu item or key. (The
    /// queue is on Now Playing only.)
    pub pane_lyrics: bool,
    pub pane_info: bool,
    /// The optional buttons of Now Playing and the player bar that show.
    pub player_buttons: PlayerButtons,
    /// «Главная» and «Библиотека» of the rail open the section's first page
    /// (the library's «Всё»), not the one left there.
    pub nav_root: bool,
    /// Deleting moves tracks into the Trash for 30 days; off: they are
    /// deleted for good at once (after a confirmation).
    pub use_trash: bool,
    /// Release groups whose editions the user chose to keep apart.
    pub kept_editions: Vec<String>,

    // ---- Sources (stage 3, the "Источники" settings tab). No secrets
    // here: the Spotify Client ID and YouTube cookies live in settings.toml
    // ([`Command::SetStreamSecrets`]).
    /// Spotify is used (search, likes, sound when the account has it).
    pub spotify_enabled: bool,
    /// YouTube Music is used.
    pub youtube_enabled: bool,
    /// The Deezer catalogue is used (search, albums, discographies; no
    /// account, its tracks play from YouTube Music).
    pub deezer_enabled: bool,
    /// SoundCloud is used (search, pages and sound work without an account).
    pub soundcloud_enabled: bool,
    /// What a like of a stream track downloads: the track (from its
    /// album's torrent) or the whole album. An album like is always the album.
    pub like_download_scope: WishlistScope,
    /// Likes download at all («Ничего» turns it off: a like is only a like;
    /// «Скачать» still downloads).
    pub like_downloads: bool,
    /// Sync mode of a playlist newly linked to a service.
    pub playlist_sync_default: PlaylistSyncMode,
    /// The playlists (and «Любимые», [`LIKED_PLAYLIST`]) whose chip «✦ С
    /// дополнением» is on: new songs join them as they play.
    pub plus_playlists: Vec<PlaylistId>,
    /// Likes and unlikes here go to the services the track is in.
    pub push_likes: bool,
    /// A like of an album likes all its tracks too (into «Любимые» and the
    /// services). Off: it only saves the album.
    pub album_like_tracks: bool,
    /// The saved albums sit among the playlists. Off: a shelf of their own,
    /// «Любимые альбомы».
    pub liked_albums_with_playlists: bool,
    /// A like of a single's song saves the single as an album too.
    pub single_like_saves: bool,
    /// «Песни из альбомов → Альбом показывает»: every album whole (the
    /// default); an album added to the library whole, one not added the
    /// user's songs («Автоматически»); every album the user's songs only.
    pub album_mode: AlbumMode,
    /// Which default of [`Self::album_mode`] the prefs were written with
    /// ([`ALBUM_REV`]): before 1 it was «Автоматически», and that turns
    /// into «Всегда весь альбом» once.
    pub album_rev: u8,
    /// «Другие версии (инструментал, ремиксы)»: the other versions of an
    /// album's songs the user did not choose play with it (karaoke only
    /// when the album's own switch says so). Off by default.
    pub album_other_versions: bool,
    /// An import of export files reads the songs saved in the music library
    /// (Takeout's «music library songs»): they teach the taste only, no
    /// likes, no downloads, not in the library. Off: that file is skipped.
    pub import_saved_songs: bool,
    /// The library lists only tracks with a file (the inconspicuous switch).
    pub library_only_downloaded: bool,
    /// How many wishlist items search / download at once on their own.
    pub auto_download_limit: u32,
    /// The download sources in the order the wishlist prefers them at one
    /// quality, each on or off («Источники скачивания»).
    /// A source missing here takes its default place, on.
    pub download_sources: Vec<SourceSwitch>,
    /// Toasts about downloads going on by themselves: a release not found, a
    /// torrent started or finished. Off: that is only in «Загрузки» and «Хочу
    /// скачать».
    pub download_toasts: bool,
    /// «Искать качество лучше»: how often the library's files below
    /// [`Self::upgrade_below`] are searched for a better copy.
    pub upgrade_every: UpgradeEvery,
    /// What is good enough: a file below it is searched for again.
    pub upgrade_below: UpgradeBar,
    /// A better copy found replaces the file by itself. Off: it waits in
    /// «Подтверди» (a release can differ from the one the user has).
    pub upgrade_auto: bool,
    /// Size limit of the stream audio cache per service, in MB (0 = off):
    /// recently played YouTube files and Spotify's encrypted files.
    pub stream_cache_mb: u32,
    /// settings.toml has a personal Spotify Client ID (the id is not here).
    pub spotify_personal_app: bool,
    /// settings.toml has YouTube Music cookies.
    pub youtube_cookies_set: bool,
    /// «Найти песню…»: the last scope chosen, «И в YouTube Music» (true) or
    /// «Только здесь».
    pub replace_in_youtube: bool,

    // ---- Recommendations (stage 4).
    /// «Новое в радио»: how much music new to the user radios and mixes mix
    /// in.
    pub radio_new_share: RadioNewShare,
    /// «Миксы только из скачанного»: mixes, moods, tiles, the daylist and
    /// radios pick only tracks with a file (no streams, no discoveries).
    pub mixes_downloaded_only: bool,
    /// «Миксы для тебя» → «С новым»: about every fifth song of a mix is new
    /// (from the services); «Только моё» (false) plays only the library.
    pub mixes_with_new: bool,
    /// «Карточка наверху главной» («Миры»): off, Главная starts with the
    /// greeting.
    pub home_card: bool,
    /// «Как к тебе обращаться»: the name in the greeting of Главная («Добрый
    /// вечер, Ник»); empty, the greeting alone ([`user_name`]).
    pub user_name: String,
    /// Главная asks for the name once: set on the first start (no prefs
    /// file before), gone with «Готово» or «Пропустить».
    pub name_prompt: bool,
    /// The first run is still to be shown ([`first_run`]): set on the
    /// first start (no prefs file before), gone when it is finished or
    /// closed; a prefs file from before it reads false, so an upgrade never
    /// sees it. Настройки → «Для разработчика» shows it again.
    pub first_run: bool,
    /// The first run was on screen and is not over: a start after it
    /// went away (the app quit midway) shows it again, whatever its own
    /// sign-ins and folder brought ([`first_run::due`]).
    pub first_run_started: bool,
    /// The worlds taken off the card's row («Убрать из ряда»), by key.
    pub home_hidden_worlds: Vec<String>,
    /// «Анализ звука стримов»: liked and playlist tracks without a file are
    /// analysed from YouTube Music in the background.
    pub stream_analysis: bool,
    /// «Нейросетевой анализ звука» as the user picked it; `None` until
    /// then, which means [`SoundNeural::LibraryAndPreviews`] when the model
    /// is already downloaded and [`SoundNeural::Off`] when it is not (no
    /// download without a choice): [`Self::sound_neural_or`].
    pub sound_neural: Option<SoundNeural>,
    /// «На чём считать анализ звука» ([`SoundDevice`]). Prefs from before
    /// it with «Только на процессоре» (`sound_cpu_only`) read as
    /// [`SoundDevice::Cpu`].
    pub sound_device: SoundDevice,
    /// «Автовоспроизведение»: when the queue's last track plays, it goes on
    /// with similar tracks by itself (a radio of that track, from where
    /// [`Self::mixes_downloaded_only`] and [`Self::radio_new_share`] say).
    pub autoplay: bool,
    /// «Продолжать мир после альбома»: after an album, autoplay plays the
    /// next tracks of its world (a soundtrack franchise, an artist's
    /// discography) in album order first, then the similar ones.
    pub autoplay_continue_world: bool,
    /// «Трек из поиска»: a song started from the search results plays
    /// alone, autoplay going on from it ([`SearchPlay::Radio`]), or with
    /// the other results after it.
    pub search_play: SearchPlay,
    /// The share of new music by the kind of radio (a track's, an
    /// artist's, a mix, autoplay): what was chosen for it last; a kind not
    /// here takes [`Self::radio_new_share`].
    pub radio_new_share_by: std::collections::BTreeMap<RadioKind, RadioNewShare>,

    // ---- Dislikes.
    /// «Кнопка «Не нравится»» next to ♡ in Now Playing.
    pub dislike_button: bool,
    /// «Пропускать по дизлайку»: a dislike of the playing track skips it.
    pub dislike_skips: bool,
    /// «Пропускать и в своём»: disliked tracks and artists not recommended
    /// are skipped in the user's own albums and playlists too (off: only in
    /// mixes, radios and autoplay).
    pub dislike_skip_everywhere: bool,
    /// «Учитывать в рекомендациях»: a dislike lowers its nearest sound
    /// neighbours a little too (off: only the track itself is left out).
    pub dislike_in_recs: bool,

    // ---- Taste profiles.
    /// «Фон меньше влияет на рекомендации»: a play of a background session
    /// counts a quarter in the taste profiles' balance (off: fully).
    pub background_counts_less: bool,
    /// «Делить профили по звуку»: a kind of song is two profiles when its
    /// sounds part (off: one profile a kind).
    pub profiles_split: bool,
    /// «Названия профилей»: by the kind of song, or by the artists.
    pub profile_names: ProfileNames,

    // ---- «Миксы для тебя».
    /// The profiles' own mixes («Русские песни»).
    pub mixes_profiles: bool,
    /// «Всё вперемешку».
    pub mixes_everything: bool,
    /// The calm and the lively side of everything loved («Спокойное»,
    /// «Погромче»).
    pub mixes_moods: bool,
    /// «На повторе».
    pub mixes_on_repeat: bool,
    /// «Карточки в „Миксах для тебя“» is one choice kept in these two (the
    /// settings' row reads and writes both): off here is «Сразу играют
    /// микс», a click plays the mix (its page from ⋮); on, a click opens
    /// its page ([`Route::Mix`]; its ▶ plays it) or starts its wave…
    pub mix_click_opens_list: bool,
    /// …«Запускают Волну с этим вайбом» when this is on too: a card of the
    /// row starts «Моя волна» with its own filter (its list stays in ⋮);
    /// off: «Открывают готовый список треков».
    pub mix_cards_wave: bool,
    /// The novelty «Моя волна» starts with: the last one chosen.
    pub wave_novelty: WaveNovelty,
    /// «Фон» cards at most (0: none), for a user who listens in the
    /// background.
    pub background_cards: u8,
    /// «Не предлагать этот микс»: the mixes of «Миксы для тебя» left out,
    /// with the name they had (Settings → «Рекомендации» returns them).
    pub home_hidden_mixes: Vec<HiddenMix>,

    // ---- The graph of similar music.
    /// «Похожее по слушателям»: what other listeners hear together (the
    /// graph kept in the library) scores the radio, the mixes and the new
    /// music; off, the graph is kept but not used.
    pub graph_use: bool,
    /// «Спрашивать YouTube Music и Deezer»: the graph asks their radios
    /// and similar artists in the background.
    pub graph_services: bool,
    /// «Спрашивать ListenBrainz и MusicBrainz»: the graph asks their open
    /// data in the background.
    pub graph_open_data: bool,
    /// «Мои плейлисты и импорт истории»: the personal graph (the order of
    /// the playlists, the sessions of an imported history).
    pub graph_personal: bool,
    /// «Без истории — по самой библиотеке»: while the history is short the
    /// library itself is the taste.
    pub library_as_taste: bool,
    /// settings.toml has a Last.fm API key (the key is not here).
    pub lastfm_configured: bool,
    /// settings.toml has a ListenBrainz token (the token is not here).
    pub listenbrainz_configured: bool,
    /// «Не повторять сыгранное»: the radio and autoplay leave out what
    /// played this many hours ago or less (and in the session) while
    /// anything else is left.
    pub radio_no_repeat_hours: u8,
    /// «Подмешивать случайное»: how much of a radio of a track or an artist
    /// (and autoplay) may be new music not known to sound like what it
    /// grows from.
    pub radio_explore: RadioExplore,
    /// «Преимущественно новые песни»: how much of a radio of a track or an
    /// artist (and autoplay) is music the user has not heard.
    pub radio_unheard: RadioUnheard,

    // ---- Settings v2 (presets, «Просто» and «Подробно»).
    /// The settings show every row («Подробно»); off, the main ones
    /// («Просто»).
    pub settings_deep: bool,
    /// The values of the user's own an area had before a preset was
    /// applied over them, for «Свои» ([`presets::Area::custom`]).
    pub custom_presets: std::collections::BTreeMap<presets::Area, presets::Snapshot>,
    /// Which defaults of the settings the prefs were written with
    /// ([`presets::SETTINGS_REV`]); older prefs keep their values once
    /// ([`presets::upgrade`]).
    pub settings_rev: u8,
    /// «Обложка → Скорость»: seconds a turn of the cover's outline takes
    /// ([`COVER_SPIN_S`]).
    pub cover_spin_s: f32,
    /// «Пульсация → Сила»: how much the cover swells on a beat, a share of
    /// its size ([`COVER_PULSE`]).
    pub cover_pulse_strength: f32,
    /// «Разделы главной»: the sections of Главная in the user's order, each
    /// on or off; empty: all of them in the default order
    /// ([`Prefs::home_order`]).
    pub home_sections: Vec<HomeSectionSwitch>,
    /// «Меню трека»: the items of a track's menu taken from their default
    /// places ([`TrackMenuItem::default_place`]); empty: the menu as it is
    /// by default ([`Prefs::track_menu_place`]).
    pub track_menu: Vec<TrackMenuSlot>,
    /// «Сохранять обработанные тексты, даже неудачные»: the word timing of
    /// a text that does not fit the recording is kept with its verdict
    /// (never made again, synced to the other devices); shown as before.
    pub lyrics_keep_misfits: bool,
}

/// The default of [`Prefs::cover_spin_s`] and its range in the settings.
pub const COVER_SPIN_S: f32 = 48.0;
pub const COVER_SPIN_RANGE: std::ops::RangeInclusive<f32> = 12.0..=90.0;
/// Seconds a turn of the ring around the cover takes ([`Prefs::ring`]).
pub const RING_SPIN_S: f32 = 36.0;
/// The default of [`Prefs::cover_pulse_strength`] and its range.
pub const COVER_PULSE: f32 = 0.08;
pub const COVER_PULSE_RANGE: std::ops::RangeInclusive<f32> = 0.02..=0.15;
/// The range of [`Prefs::ui_scale`] and its step (80–150 % by 10).
pub const UI_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.8..=1.5;
pub const UI_SCALE_STEP: f32 = 0.1;

/// `scale` on a step of [`UI_SCALE_STEP`] inside [`UI_SCALE_RANGE`] (a
/// value that is not a number: 100 %).
pub fn ui_scale(scale: f32) -> f32 {
    if !scale.is_finite() {
        return 1.0;
    }
    // Divided rather than multiplied: 1.1 comes out as the 1.1 written.
    let per = (1.0 / UI_SCALE_STEP).round();
    ((scale * per).round() / per).clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end())
}

/// The scale `steps` steps of [`UI_SCALE_STEP`] from `scale` (Ctrl+= one
/// up, Ctrl+- one down), inside the range.
pub fn ui_scale_step(scale: f32, steps: i32) -> f32 {
    let per = (1.0 / UI_SCALE_STEP).round();
    ui_scale(((ui_scale(scale) * per).round() + steps as f32) / per)
}

/// An item of a track's menu whose place «Меню трека» chooses. The names
/// are kept in the prefs: an item is never renamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackMenuItem {
    Radio,
    Album,
    Artist,
    Share,
    /// «Источник и качество».
    Info,
    /// «Скорость и эквалайзер» of the playing track, «Скорость…» of a row.
    Sound,
    Visualizer,
    /// «Редактировать теги…».
    Tags,
    /// «Изменить текст…».
    LyricsEdit,
    /// «Скачать» of a track with no file («Скачать альбом» stays in
    /// «Файл»).
    Download,
    /// «Найти качество лучше» of a track with a file.
    Torrents,
    /// «Выбрать раздачу…» of a track with no file (the indexer's search).
    /// Until it is placed it stands where [`Self::Torrents`] does: the two
    /// were one item before.
    PickRelease,
    /// «В файле другая песня».
    WrongFile,
    Dislike,
    /// «Не рекомендовать исполнителя».
    Ban,
}

impl TrackMenuItem {
    /// Every item, in the order of the menu.
    pub const ALL: [Self; 15] = [
        Self::Radio,
        Self::Album,
        Self::Artist,
        Self::Share,
        Self::Info,
        Self::Sound,
        Self::Visualizer,
        Self::Tags,
        Self::LyricsEdit,
        Self::Torrents,
        Self::WrongFile,
        Self::Download,
        Self::PickRelease,
        Self::Dislike,
        Self::Ban,
    ];

    /// The item's name in the settings in `loc`.
    pub const fn label_in(self, loc: Locale) -> &'static str {
        let v = view_in(loc);
        match self {
            Self::Radio => v.menu_radio,
            Self::Album => v.menu_album,
            Self::Artist => v.menu_artist,
            Self::Share => v.menu_share,
            Self::Info => v.menu_info,
            Self::Sound => v.menu_sound,
            Self::Visualizer => v.menu_visualizer,
            Self::Tags => v.menu_tags,
            Self::LyricsEdit => v.menu_lyrics,
            Self::Download => v.menu_download,
            Self::Torrents => v.menu_torrents,
            Self::PickRelease => v.menu_pick_release,
            Self::WrongFile => v.menu_wrong_file,
            Self::Dislike => v.menu_dislike,
            Self::Ban => v.menu_ban,
        }
    }

    /// The item's name in the settings in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(locale())
    }

    /// Where the item stands until the settings say otherwise.
    pub fn default_place(self) -> TrackMenuPlace {
        match self {
            Self::Info | Self::Tags | Self::LyricsEdit | Self::Torrents | Self::PickRelease | Self::WrongFile => {
                TrackMenuPlace::File
            }
            _ => TrackMenuPlace::Menu,
        }
    }
}

/// Where an item of a track's menu stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackMenuPlace {
    /// In the menu itself.
    Menu,
    /// In its «Файл» submenu.
    File,
    Hidden,
}

impl TrackMenuPlace {
    pub const ALL: [Self; 3] = [Self::Menu, Self::File, Self::Hidden];
}

/// One line of [`Prefs::track_menu`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrackMenuSlot {
    pub item: TrackMenuItem,
    pub place: TrackMenuPlace,
}

impl TrackMenuSlot {
    /// Where `item` stands by `slots` (the last line about it counts).
    pub fn place_of(slots: &[Self], item: TrackMenuItem) -> TrackMenuPlace {
        let own = |item: TrackMenuItem| slots.iter().rev().find(|s| s.item == item).map(|s| s.place);
        match (own(item), item) {
            (Some(place), _) => place,
            // Saved while «Выбрать раздачу…» and «Найти качество лучше»
            // were one item: it goes where that one went.
            (None, TrackMenuItem::PickRelease) => own(TrackMenuItem::Torrents).unwrap_or(item.default_place()),
            (None, _) => item.default_place(),
        }
    }
}

impl Prefs {
    /// Where `item` of a track's menu stands.
    pub fn track_menu_place(&self, item: TrackMenuItem) -> TrackMenuPlace {
        TrackMenuSlot::place_of(&self.track_menu, item)
    }

    /// Puts `item` at `place`; only what differs from the default is kept.
    pub fn set_track_menu_place(&mut self, item: TrackMenuItem, place: TrackMenuPlace) {
        // «Найти качество лучше» placed while «Выбрать раздачу…» has no
        // line of its own: the latter keeps where it stands now (where a
        // layout saved before the split put both, or its default) on a line
        // of its own, so moving one never moves the other. Only a layout
        // saved before the split has a `torrents` line without a
        // `pick_release` one, and only there does [`TrackMenuSlot::place_of`]
        // fall back.
        if item == TrackMenuItem::Torrents && !self.track_menu.iter().any(|s| s.item == TrackMenuItem::PickRelease) {
            let now = self.track_menu_place(TrackMenuItem::PickRelease);
            self.track_menu.push(TrackMenuSlot { item: TrackMenuItem::PickRelease, place: now });
        }
        self.track_menu.retain(|s| s.item != item);
        if place != item.default_place() || (item == TrackMenuItem::PickRelease && self.track_menu.iter().any(|s| s.item == TrackMenuItem::Torrents)) {
            self.track_menu.push(TrackMenuSlot { item, place });
        }
        // With no `torrents` line nothing falls back: a `pick_release` line
        // at its default says nothing.
        if !self.track_menu.iter().any(|s| s.item == TrackMenuItem::Torrents) {
            self.track_menu.retain(|s| !(s.item == TrackMenuItem::PickRelease && s.place == TrackMenuItem::PickRelease.default_place()));
        }
    }
}

/// A row of Главная that gets more cards at its end ([`Command::HomeMore`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HomeRow {
    /// «Миксы для тебя».
    Mixes,
    /// «Слушать снова».
    ListenAgain,
    /// «Новое для тебя».
    New,
}

/// A section of Главная («Разделы главной»).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HomeSection {
    /// «Быстрый доступ».
    Shortcuts,
    /// «Миксы дня», a row there no longer is (its mixes stand in «Миксы
    /// для тебя»): kept so that a list saved with it still reads; it is in
    /// no order and no list ([`HomeSectionSwitch::complete`]).
    Daily,
    /// «Миксы для тебя».
    Mixes,
    /// «Слушать снова».
    ListenAgain,
    /// «Плитки дня».
    OfDay,
    /// «Из библиотеки».
    FromLibrary,
    /// «Новое».
    New,
    /// «Релизы».
    Releases,
}

impl HomeSection {
    /// Every section in Главная's default order.
    pub const ALL: [Self; 7] = [
        Self::Shortcuts,
        Self::Mixes,
        Self::ListenAgain,
        Self::OfDay,
        Self::FromLibrary,
        Self::New,
        Self::Releases,
    ];

    pub const fn label_in(self, loc: Locale) -> &'static str {
        let v = view_in(loc);
        match self {
            Self::Shortcuts => v.home_shortcuts,
            Self::Daily => v.home_daily,
            Self::Mixes => v.home_mixes,
            Self::ListenAgain => v.home_listen_again,
            Self::OfDay => v.home_of_day,
            Self::FromLibrary => v.home_from_library,
            Self::New => v.home_new,
            Self::Releases => v.home_releases,
        }
    }

    /// The section's name in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(locale())
    }
}

/// One line of [`Prefs::home_sections`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomeSectionSwitch {
    pub key: HomeSection,
    pub on: bool,
}

impl Prefs {
    /// Every section of Главная in the user's order, the ones not listed
    /// yet in their default places (on).
    pub fn home_order(&self) -> Vec<HomeSectionSwitch> {
        HomeSectionSwitch::complete(&self.home_sections)
    }

    /// Whether Главная shows `section` (when it has anything for it).
    pub fn home_section_on(&self, section: HomeSection) -> bool {
        self.home_sections.iter().find(|s| s.key == section).is_none_or(|s| s.on)
    }
}

impl HomeSectionSwitch {
    /// `list` with each section once and every one there: the missing ones
    /// in their default places, on.
    pub fn complete(list: &[Self]) -> Vec<Self> {
        let mut out: Vec<Self> = Vec::new();
        for s in list {
            if HomeSection::ALL.contains(&s.key) && !out.iter().any(|o| o.key == s.key) {
                out.push(*s);
            }
        }
        for (i, k) in HomeSection::ALL.iter().enumerate() {
            if out.iter().any(|o| o.key == *k) {
                continue;
            }
            // After the default sections that come before it.
            let at = HomeSection::ALL[..i]
                .iter()
                .filter_map(|prev| out.iter().position(|o| o.key == *prev))
                .max()
                .map_or(0, |p| p + 1);
            out.insert(at, Self { key: *k, on: true });
        }
        out
    }
}

/// One line of [`Prefs::download_sources`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceSwitch {
    pub key: SourceKey,
    pub on: bool,
}

impl Prefs {
    /// Every source in the user's order, the ones not listed yet in their
    /// default places (on, Soulseek off).
    pub fn source_order(&self) -> Vec<SourceSwitch> {
        let mut out: Vec<SourceSwitch> = Vec::new();
        for s in &self.download_sources {
            if !out.iter().any(|o| o.key == s.key) {
                out.push(*s);
            }
        }
        for (i, k) in SourceKey::ALL.iter().enumerate() {
            if out.iter().any(|o| o.key == *k) {
                continue;
            }
            // After the default sources that come before it.
            let at = SourceKey::ALL[..i]
                .iter()
                .filter_map(|prev| out.iter().position(|o| o.key == *prev))
                .max()
                .map_or(0, |p| p + 1);
            // Soulseek needs the user's own account, Squidify's owner
            // keeps downloads closed to guests, and SoundCloud's originals
            // come through the user's own SoundCloud sign-in: off until
            // switched on.
            out.insert(at, SourceSwitch { key: *k, on: !matches!(*k, SourceKey::Soulseek | SourceKey::Squidify | SourceKey::Soundcloud) });
        }
        out
    }

    /// Whether `key` is switched on.
    pub fn source_on(&self, key: SourceKey) -> bool {
        self.source_order().iter().find(|s| s.key == key).is_none_or(|s| s.on)
    }

    /// Prefs written with the lyrics defaults of revision `stored` (0: from
    /// before [`Prefs::lyrics_rev`]) take the newer defaults once: those
    /// switches were never on the settings page under their new meaning,
    /// so what they hold is the old default, not a choice.
    pub fn upgrade_lyrics(&mut self, stored: u8) {
        if stored < 1 {
            // Recognition from scratch is experimental: off unless chosen.
            self.lyrics_recognize = false;
            self.lyrics_words_all = false;
            self.lyrics_words_playing = false;
            self.lyrics_lines_auto = true;
        }
        if stored < 2 && self.lyrics_ui == 0 {
            // «Окно» became the default: the old default moves, a chosen
            // window or page stays.
            self.lyrics_ui = 1;
        }
        if stored < 3 {
            // Word highlighting became the default experience: the
            // playing song's texts are shown by words the player made
            // (the switch was off by default, never a choice when off).
            self.lyrics_words_playing = true;
            // The lead moved to what karaoke apps use; one the user set
            // stays.
            if (self.lyrics_lead - OLD_LYRICS_LEAD).abs() < 1e-3 {
                self.lyrics_lead = LYRICS_LEAD;
            }
        }
        if stored < 4 {
            // «По слогам» became the default unit: «По буквам» (the old
            // default) moves to it, «По словам» chosen stays.
            self.lyrics_unit = if self.lyrics_by_words || self.lyrics_style == 3 { LYRICS_BY_WORDS } else { LYRICS_BY_SYLLABLES };
        }
        if stored < 5 {
            // The player's lyrics button is hidden by default now (the key
            // L opens the lyrics): it goes once; shown again since, it stays.
            self.player_buttons.lyrics = false;
        }
        if stored < 6 {
            // The lyrics button is back by default: it shows once; hidden
            // again since, it stays hidden.
            self.player_buttons.lyrics = true;
        }

        self.lyrics_rev = LYRICS_REV;
        self.upgrade_lyrics_style();
    }

    /// «По словам» was a fourth animation (`lyrics_style` 3) before it
    /// became the unit of every animation: it moves to the wave by words,
    /// once (3 is never written again).
    pub fn upgrade_lyrics_style(&mut self) {
        if self.lyrics_style == 3 {
            self.lyrics_style = 2;
            self.lyrics_by_words = true;
            self.lyrics_unit = LYRICS_BY_WORDS;
        }
    }
}

/// A mix of «Миксы для тебя» left out ([`Prefs::home_hidden_mixes`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HiddenMix {
    pub key: MixKey,
    pub title: String,
}

/// What the taste profiles are called by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProfileNames {
    /// «Японские песни», the artists below.
    #[default]
    Kind,
    /// «Eve, Radwimps и другие».
    Artists,
}

impl Prefs {
    /// The share of new music of a radio of `kind`.
    pub fn new_share_of(&self, kind: RadioKind) -> RadioNewShare {
        self.radio_new_share_by.get(&kind).copied().unwrap_or(self.radio_new_share)
    }
}

/// What a radio grows from, for the share of new music it remembers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum RadioKind {
    #[default]
    Track,
    Artist,
    Mix,
    Autoplay,
}

/// One preference changed on its own ([`Command::SetPref`]): the runtime
/// applies it to the preferences as they are when it comes, so a copy the
/// view held from before another change cannot undo that change.
#[derive(Clone, Debug, PartialEq)]
pub enum PrefPatch {
    /// Interface language (`Auto` follows the OS language).
    Language(LangPref),
    /// «Миксы для тебя»: «С новым» (true) or «Только моё».
    MixesWithNew(bool),
    /// «Миксы только из скачанного» («Только скачанное» on Главная).
    MixesDownloadedOnly(bool),
    /// The library lists only tracks with a file.
    LibraryOnlyDownloaded(bool),
    /// «Автовоспроизведение».
    Autoplay(bool),
    /// «Продолжать мир после альбома».
    AutoplayContinueWorld(bool),
    /// Where a radio takes its tracks («Скачанное · Библиотека · С новым»):
    /// both of the fields it sets.
    RadioSource { downloaded_only: bool, new_share: RadioNewShare },
    /// A world of the card at the top of Главная taken off its row.
    HideWorld(String),
    /// The card at the top of Главная shown or hidden.
    HomeCard(bool),
    /// The outline the Now Playing cover takes.
    CoverShape(CoverShape),
    /// «Менять форму на паузе».
    CoverPauseMorph(bool),
    /// «Дата альбома».
    AlbumDate(AlbumDate),
    /// What stands beside the Now Playing cover.
    NpSlot(NpSlot),
    /// The visualizer's picture.
    VizConcept(VizConcept),
    /// The visualizer on the whole screen or in the window.
    VizFullscreen(bool),
    /// Windows starts the app at sign-in (as the system says).
    Autostart(bool),
    /// «Как к тебе обращаться» ([`user_name`] keeps it tidy); Главная asks
    /// no more.
    UserName(String),
    /// Главная's question of the name: `false` («Пропустить») asks no more.
    NamePrompt(bool),
    /// The first run is shown (`true`: «Показать первый запуск») or gone
    /// (and no more started).
    FirstRun(bool),
    /// The first run came on screen ([`Prefs::first_run_started`]).
    FirstRunStarted,
    /// The share of new music of one kind of radio (the source buttons
    /// above the queue).
    RadioNewShareBy(RadioKind, RadioNewShare),
    /// The share of new music everywhere: each kind of radio takes it.
    RadioNewShare(RadioNewShare),
    DislikeButton(bool),
    DislikeSkips(bool),
    DislikeSkipEverywhere(bool),
    DislikeInRecs(bool),
    BackgroundCountsLess(bool),
    ProfilesSplit(bool),
    ProfileNames(ProfileNames),
    MixesProfiles(bool),
    MixesEverything(bool),
    MixesMoods(bool),
    MixesOnRepeat(bool),
    BackgroundCards(u8),
    /// «Не предлагать этот микс».
    HideMix(HiddenMix),
    /// A mix left out comes back.
    ShowMix(MixKey),
    GraphUse(bool),
    GraphServices(bool),
    GraphOpenData(bool),
    GraphPersonal(bool),
    LibraryAsTaste(bool),
    RadioNoRepeat(u8),
    RadioExplore(RadioExplore),
    RadioUnheard(RadioUnheard),
    SoundNeural(SoundNeural),
    SoundDevice(SoundDevice),
    /// «Настройки текста…» was picked from the lyrics control's menu.
    LyricsSettingsSeen,
    /// A preset of an area by its number ([`presets::Area::apply`]: the
    /// user's own values are kept first when the area has them).
    Preset(presets::Area, usize),
    /// «Свои»: the values kept before a preset come back.
    PresetCustom(presets::Area),
    /// «Просто» (false) or «Подробно».
    SettingsDeep(bool),
    /// «Разделы главной»: the order and switches of every section.
    HomeSections(Vec<HomeSectionSwitch>),
    /// «Масштаб интерфейса» from the keys (Ctrl+=, Ctrl+-, Ctrl+0); kept
    /// on a step inside the range ([`ui_scale`]).
    UiScale(f32),
    /// The chip «✦ С дополнением» of a playlist on or off.
    Plus { playlist: PlaylistId, on: bool },
    /// What the settings page changed on its copy of the prefs: every
    /// field `after` has unlike `before`, laid over the prefs as they are
    /// when it comes ([`Prefs::merge_edit`]), so a copy older than a change
    /// still on its way never takes that change back.
    Edit { before: Box<Prefs>, after: Box<Prefs> },
}

impl PrefPatch {
    /// The change made to `p`.
    pub fn apply(&self, p: &mut Prefs) {
        match self {
            Self::Language(lang) => p.language = *lang,
            Self::MixesWithNew(on) => p.mixes_with_new = *on,
            Self::MixesDownloadedOnly(on) => p.mixes_downloaded_only = *on,
            Self::LibraryOnlyDownloaded(on) => p.library_only_downloaded = *on,
            Self::Autoplay(on) => p.autoplay = *on,
            Self::AutoplayContinueWorld(on) => p.autoplay_continue_world = *on,
            Self::RadioSource { downloaded_only, new_share } => {
                p.mixes_downloaded_only = *downloaded_only;
                p.radio_new_share = *new_share;
            }
            Self::HideWorld(key) => {
                if !p.home_hidden_worlds.contains(key) {
                    p.home_hidden_worlds.push(key.clone());
                }
            }
            Self::HomeCard(on) => p.home_card = *on,
            Self::CoverShape(s) => p.cover_shape = *s,
            Self::CoverPauseMorph(on) => p.cover_pause_morph = *on,
            Self::AlbumDate(d) => p.album_date = *d,
            Self::NpSlot(s) => p.np_slot = *s,
            Self::VizConcept(c) => p.viz_concept = *c,
            Self::VizFullscreen(on) => p.viz_fullscreen = *on,
            Self::Autostart(on) => p.autostart = *on,
            Self::UserName(name) => {
                p.user_name = user_name(name);
                p.name_prompt = false;
            }
            Self::NamePrompt(on) => p.name_prompt = *on,
            Self::FirstRun(on) => {
                p.first_run = *on;
                p.first_run_started &= *on;
            }
            Self::FirstRunStarted => p.first_run_started = true,
            Self::RadioNewShareBy(kind, share) => {
                p.radio_new_share_by.insert(*kind, *share);
            }
            Self::RadioNewShare(share) => {
                p.radio_new_share = *share;
                p.radio_new_share_by.clear();
            }

            Self::DislikeButton(on) => p.dislike_button = *on,
            Self::DislikeSkips(on) => p.dislike_skips = *on,
            Self::DislikeSkipEverywhere(on) => p.dislike_skip_everywhere = *on,
            Self::DislikeInRecs(on) => p.dislike_in_recs = *on,
            Self::BackgroundCountsLess(on) => p.background_counts_less = *on,
            Self::ProfilesSplit(on) => p.profiles_split = *on,
            Self::ProfileNames(n) => p.profile_names = *n,
            Self::MixesProfiles(on) => p.mixes_profiles = *on,
            Self::MixesEverything(on) => p.mixes_everything = *on,
            Self::MixesMoods(on) => p.mixes_moods = *on,
            Self::MixesOnRepeat(on) => p.mixes_on_repeat = *on,
            Self::BackgroundCards(n) => p.background_cards = (*n).min(3),
            Self::HideMix(m) => {
                if !p.home_hidden_mixes.iter().any(|h| h.key == m.key) {
                    p.home_hidden_mixes.push(m.clone());
                }
            }
            Self::ShowMix(key) => p.home_hidden_mixes.retain(|h| h.key != *key),
            Self::GraphUse(on) => p.graph_use = *on,
            Self::GraphServices(on) => p.graph_services = *on,
            Self::GraphOpenData(on) => p.graph_open_data = *on,
            Self::GraphPersonal(on) => p.graph_personal = *on,
            Self::LibraryAsTaste(on) => p.library_as_taste = *on,
            Self::LyricsSettingsSeen => p.lyrics_settings_seen = true,
            Self::RadioNoRepeat(h) => p.radio_no_repeat_hours = *h,
            Self::RadioExplore(e) => p.radio_explore = *e,
            Self::RadioUnheard(u) => p.radio_unheard = *u,
            Self::SoundNeural(m) => p.sound_neural = Some(*m),
            Self::SoundDevice(d) => p.sound_device = *d,
            Self::Preset(area, n) => area.apply(p, *n),
            Self::PresetCustom(area) => {
                if let Some(back) = area.custom(p) {
                    *p = back;
                }
            }
            Self::SettingsDeep(on) => p.settings_deep = *on,
            Self::HomeSections(list) => {
                // Each section once, every one there, in the order given.
                p.home_sections = HomeSectionSwitch::complete(list);
            }
            Self::UiScale(scale) => p.ui_scale = ui_scale(*scale),
            Self::Plus { playlist, on } => {
                p.plus_playlists.retain(|x| x != playlist);
                if *on {
                    p.plus_playlists.push(*playlist);
                }
            }
            Self::Edit { before, after } => p.merge_edit(before, after),
        }
    }
}

/// The longest name the greeting takes.
pub const USER_NAME_MAX: usize = 32;

/// A name as the greeting keeps it: trimmed, its spaces single, at most
/// [`USER_NAME_MAX`] characters.
pub fn user_name(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(USER_NAME_MAX).collect::<String>().trim_end().to_owned()
}

/// «Новое в радио»: the share of new music in radios and mixes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioNewShare {
    /// «Без нового».
    None,
    /// «Немного», about a fifth.
    #[default]
    Some,
    /// «Побольше», about two fifths.
    More,
}

impl RadioNewShare {
    /// The share of a queue that is new music.
    pub const fn fraction(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Some => 0.2,
            Self::More => 0.4,
        }
    }
}

/// «Подмешивать случайное»: how often a radio of a track or an artist (and
/// autoplay) plays new music not known to sound like what it grows from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioExplore {
    /// «Никогда»: only new music that sounds like it.
    Never,
    /// «Редко», about one in ten.
    #[default]
    Rare,
    /// «Иногда», about one in five (as every radio did before).
    Sometimes,
    /// «Часто», about three in ten.
    Often,
}

impl RadioExplore {
    pub const ALL: [Self; 4] = [Self::Never, Self::Rare, Self::Sometimes, Self::Often];

    /// The share of a radio at most that may be such new music (within
    /// its share of new music).
    pub const fn fraction(self) -> f32 {
        match self {
            Self::Never => 0.0,
            Self::Rare => 0.1,
            Self::Sometimes => 0.2,
            Self::Often => 0.3,
        }
    }
}

/// «Нейросетевой анализ звука»: what the
/// neural sound model hears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SoundNeural {
    /// «Выключен»: bliss alone, nothing downloaded, nothing loaded.
    #[default]
    Off,
    /// «Только моя библиотека»: the library's tracks in the background.
    Library,
    /// «Библиотека и превью новых песен»: and the previews of new music
    /// before it joins the mixes.
    LibraryAndPreviews,
}

/// «На чём считать анализ звука»: where the neural sound model runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SoundDevice {
    /// «Автоматически»: the CPU while it is free; the GPU while the CPU
    /// is busy and the card has free memory; the CPU again once the card
    /// is busy (decided batch by batch).
    #[default]
    Auto,
    /// «Процессор»: one quiet CPU thread, never the GPU.
    Cpu,
    /// «Видеокарта»: the GPU (DirectML), the CPU where there is none.
    Gpu,
}

impl SoundDevice {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Cpu, Self::Gpu];
}

impl SoundNeural {
    pub const ALL: [Self; 3] = [Self::Off, Self::Library, Self::LibraryAndPreviews];

    /// The library's tracks get the model's vector.
    pub const fn library(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Previews of new music get it too.
    pub const fn previews(self) -> bool {
        matches!(self, Self::LibraryAndPreviews)
    }
}

impl Prefs {
    /// [`Self::sound_neural`], or what it is before the user picks it:
    /// the full mode when the model is on disk already (`model_ready`),
    /// off otherwise.
    pub fn sound_neural_or(&self, model_ready: bool) -> SoundNeural {
        self.sound_neural.unwrap_or(if model_ready { SoundNeural::LibraryAndPreviews } else { SoundNeural::Off })
    }
}

/// The neural sound model as the settings show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundModelView {
    /// Its files are on disk.
    pub ready: bool,
    /// Its download, in percent, while one runs.
    pub downloading: Option<u8>,
    /// Its download or load failed lately.
    pub failed: bool,
    /// Library files with the model's vector, and files in all.
    pub done: u32,
    pub total: u32,
}

/// «Преимущественно новые песни»: how much of a radio of a track or an
/// artist (and autoplay) is music the user has not heard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioUnheard {
    /// «Как обычно».
    #[default]
    Usual,
    /// «Больше новых»: half of the library's picks at least.
    More,
    /// «Только новые»: all, while there are.
    Only,
}

impl RadioUnheard {
    pub const ALL: [Self; 3] = [Self::Usual, Self::More, Self::Only];

    /// The share of the library's picks at least that is unheard.
    pub const fn fraction(self) -> f32 {
        match self {
            Self::Usual => 0.0,
            Self::More => 0.5,
            Self::Only => 1.0,
        }
    }
}

/// Material shapes behind the open track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BgShapes {
    #[default]
    Off,
    /// Lying on the floor and jumping on the beats.
    Bounce,
    /// Drifting and glowing with their band.
    Float,
    /// Drifting and turning into other shapes on the beats.
    Morph,
}

/// The single content slot beside the Now Playing cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NpSlot {
    #[default]
    None,
    Lyrics,
    Queue,
    Info,
}

impl NpSlot {
    pub fn toggle(&mut self, slot: Self) {
        *self = if *self == slot { Self::None } else { slot };
    }
}

/// What follows a song started from the search results
/// ([`Prefs::search_play`]). An album, a playlist or an artist found plays
/// as its own list either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchPlay {
    /// «Радио по треку»: the song alone in the queue; autoplay (a radio of
    /// it) fills «Далее».
    #[default]
    Radio,
    /// «Остальные результаты поиска»: the results shown, from the song on.
    Results,
}

/// The ring around the Now Playing cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RingStyle {
    /// Dots on a circle.
    Circle,
    /// Dots following the cover's outline at a fixed gap.
    #[default]
    Contour,
    /// Dots around the outline that grow into bars with the spectrum
    /// (caelestia's cover visualiser).
    Bars,
}

/// What the visualizer draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VizConcept {
    /// The cover in its shape: it swells and changes shape on the beat,
    /// the spectrum stands around its outline.
    #[default]
    Cover,
    /// Mirrored bars of the spectrum with falling peaks.
    Spectrum,
    /// A morphing core with satellites, one per part of the spectrum.
    Shapes,
    /// Slow outlines spreading from a breathing shape, one per beat.
    Calm,
}

impl VizConcept {
    pub const ALL: [Self; 4] = [Self::Cover, Self::Spectrum, Self::Shapes, Self::Calm];
}

/// How much of an album's release date its page shows
/// ([`Prefs::album_date`]); what is not known falls back to what is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AlbumDate {
    /// «2026».
    #[default]
    Year,
    /// «Октябрь 2026».
    MonthYear,
    /// «23 октября 2026».
    Full,
}

/// The Now Playing cover's outline while playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CoverShape {
    /// No outline: the square cover as is.
    Original,
    /// A new random shape whenever a track starts.
    Random,
    #[default]
    Cookie12,
    Cookie9,
    Cookie7,
    Cookie6,
    Cookie4,
    Sunny,
    VerySunny,
    Flower,
    Clover4,
    Clover8,
    SoftBurst,
    SoftBoom,
    Puffy,
    PuffyDiamond,
    Circle,
    Pentagon,
    Gem,
    Heart,
}

impl CoverShape {
    pub const ALL: [Self; 20] = [
        Self::Original,
        Self::Random,
        Self::Cookie12,
        Self::Cookie9,
        Self::Cookie7,
        Self::Cookie6,
        Self::Cookie4,
        Self::Sunny,
        Self::VerySunny,
        Self::Flower,
        Self::Clover4,
        Self::Clover8,
        Self::SoftBurst,
        Self::SoftBoom,
        Self::Puffy,
        Self::PuffyDiamond,
        Self::Circle,
        Self::Pentagon,
        Self::Gem,
        Self::Heart,
    ];

    /// The shapes [`CoverShape::Random`] picks from.
    pub fn shapes() -> &'static [Self] {
        &Self::ALL[2..]
    }

    pub const fn label_in(self, loc: Locale) -> &'static str {
        let v = view_in(loc);
        match self {
            Self::Original => v.shape_original,
            Self::Random => v.shape_random,
            Self::Cookie12 => v.shape_cookie12,
            Self::Cookie9 => v.shape_cookie9,
            Self::Cookie7 => v.shape_cookie7,
            Self::Cookie6 => v.shape_cookie6,
            Self::Cookie4 => v.shape_cookie4,
            Self::Sunny => v.shape_sunny,
            Self::VerySunny => v.shape_very_sunny,
            Self::Flower => v.shape_flower,
            Self::Clover4 => v.shape_clover4,
            Self::Clover8 => v.shape_clover8,
            Self::SoftBurst => v.shape_soft_burst,
            Self::SoftBoom => v.shape_soft_boom,
            Self::Puffy => v.shape_puffy,
            Self::PuffyDiamond => v.shape_puffy_diamond,
            Self::Circle => v.shape_circle,
            Self::Pentagon => v.shape_pentagon,
            Self::Gem => v.shape_gem,
            Self::Heart => v.shape_heart,
        }
    }

    /// The shape's name in the active process locale.
    pub fn label(self) -> &'static str {
        self.label_in(locale())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Design {
    #[default]
    Md3e,
    Nothing,
    Glass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ambient {
    Off,
    Glow,
    #[default]
    Reactive,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            language: LangPref::Auto,
            design: Design::Md3e,
            theme: ThemeMode::Dark,
            contrast: 0,
            scheme_variant: 1,
            ambient: Ambient::Reactive,
            fps_limit: 60,
            reduced_motion: false,
            tooltips: false,
            gapless: true,
            crossfade_s: 0.0,
            normalization: true,
            eq_on: false,
            eq_gains: [0.0; EQ_BANDS],
            speed_enabled: true,
            keep_timbre: false,
            list_shuffle: ListShuffles::default(),
            shuffle_rev: SHUFFLE_REV,
            shuffle_radio: false,
            opus_bitrate: 160,
            check_fake_lossless: true,
            convert_fake_lossless: true,
            fake_flac_auto: false,
            library_dir: PathBuf::new(),
            inbox_dir: PathBuf::new(),
            masters_dir: PathBuf::new(),
            fallback_seed: 0xFF6750A4,
            prowlarr_url: String::new(),
            prowlarr_configured: false,
            seed_limit_gb: 50,
            seed_days: 30,
            no_seed: false,
            output_devices: Vec::new(),
            output_device: None,
            metadata_contact: String::new(),
            acoustid_configured: false,
            seed_ratio: 2.0,
            download_limit_kbps: 0,
            upload_limit_kbps: 0,
            np_slot: NpSlot::None,
            lyrics_style: 0,
            wave_syllables_only: true,
            lyrics_wave_jump: 1.0,
            lyrics_jump_speed: 1.0,
            lyrics_light_speed: 0.65,
            lyrics_smooth: 0.5,
            lyrics_dim: 0.5,
            lyrics_scale: 1.0,
            lyrics_lead: LYRICS_LEAD,
            lyrics_cascade: true,
            lyrics_glow: false,
            lyrics_lift: 0.0,
            lyrics_return_s: 4.0,
            lyrics_gap_style: 1,
            lyrics_gap_waves: 6.0,
            lyrics_gap_height: 1.0,
            lyrics_gap_size: 1.0,
            lyrics_countdown: true,
            lyrics_by_words: false,
            lyrics_unit: LYRICS_BY_SYLLABLES,
            lyrics_soft_edges: true,
            lyrics_soft_strength: 1.0,
            lyrics_soft_depth: 0.18,
            lyrics_soft_fade: 0.4,
            save_found_lyrics: false,
            // DirectML is Windows' own: the CPU is the only choice elsewhere.
            lyrics_gpu: cfg!(windows),
            lyrics_check: true,
            lyrics_words_all: false,
            lyrics_lines_auto: true,
            lyrics_words_playing: true,
            lyrics_rev: LYRICS_REV,
            lyrics_ui: 1,
            lyrics_recognize: false,
            lyrics_separate: true,
            lyrics_deep: false,
            lyrics_models_auto: true,
            lyrics_genius: true,
            lyrics_streams: true,
            lyrics_stream_keep: 0,
            lyrics_settings_seen: false,
            cover_shape: CoverShape::default(),
            cover_pause_morph: true,
            cover_spin: true,
            cover_pulse: true,
            ring: true,
            ring_style: RingStyle::default(),
            viz_concept: VizConcept::default(),
            viz_fullscreen: true,
            cover_ccw: true,
            np_volume: false,
            np_volume_pop: false,
            np_volume_pop_hold: 0.8,
            np_motion: 1,
            progress_wave: true,
            random_shapes: Vec::new(),
            autostart: false,
            autostart_hidden: true,
            tray_keep_window: true,
            cover_click_pause: true,
            dbl_click_shape: 1,
            np_tap_wait: 0.22,
            toast_s: 4,
            bg_shapes: BgShapes::Off,
            bg_shapes_count: 14,
            bg_shapes_size: 1.0,
            bg_shapes_opacity: 1.0,
            bg_shapes_speed: 1.0,
            bg_shapes_react: 1.0,
            bg_shapes_colors: 0,
            bg_shapes_hue: 5,
            ambient_strength: 1.0,
            ui_scale: 1.0,
            rail_compact: false,
            album_date: AlbumDate::Year,
            pane_lyrics: false,
            pane_info: false,
            player_buttons: PlayerButtons::default(),
            nav_root: true,
            use_trash: true,
            kept_editions: Vec::new(),
            spotify_enabled: true,
            youtube_enabled: true,
            deezer_enabled: true,
            soundcloud_enabled: true,
            like_download_scope: WishlistScope::Track,
            like_downloads: true,
            playlist_sync_default: PlaylistSyncMode::Follow,
            plus_playlists: Vec::new(),
            push_likes: true,
            album_like_tracks: false,
            liked_albums_with_playlists: false,
            single_like_saves: false,
            album_mode: AlbumMode::Whole,
            album_rev: ALBUM_REV,
            album_other_versions: false,
            import_saved_songs: true,
            library_only_downloaded: false,
            auto_download_limit: 2,
            download_sources: Vec::new(),
            download_toasts: false,
            upgrade_every: UpgradeEvery::Week,
            upgrade_below: UpgradeBar::Lossless,
            upgrade_auto: false,
            stream_cache_mb: 500,
            spotify_personal_app: false,
            youtube_cookies_set: false,
            replace_in_youtube: false,
            radio_new_share: RadioNewShare::Some,
            mixes_downloaded_only: false,
            mixes_with_new: false,
            home_card: true,
            user_name: String::new(),
            name_prompt: false,
            first_run: false,
            first_run_started: false,
            home_hidden_worlds: Vec::new(),
            home_hidden_mixes: Vec::new(),
            stream_analysis: true,
            sound_neural: None,
            sound_device: SoundDevice::Auto,
            autoplay: true,
            autoplay_continue_world: true,
            search_play: SearchPlay::Radio,
            radio_new_share_by: std::collections::BTreeMap::new(),
            dislike_button: true,
            dislike_skips: true,
            dislike_skip_everywhere: false,
            dislike_in_recs: true,
            background_counts_less: true,
            profiles_split: true,
            profile_names: ProfileNames::Kind,
            mixes_profiles: true,
            mixes_everything: true,
            mixes_moods: true,
            mixes_on_repeat: true,
            mix_click_opens_list: true,
            mix_cards_wave: true,
            wave_novelty: WaveNovelty::Balance,
            background_cards: 3,
            graph_use: true,
            graph_services: true,
            graph_open_data: true,
            graph_personal: true,
            library_as_taste: true,
            lastfm_configured: false,
            listenbrainz_configured: false,
            radio_no_repeat_hours: 3,
            radio_explore: RadioExplore::Rare,
            radio_unheard: RadioUnheard::Usual,
            settings_deep: false,
            custom_presets: std::collections::BTreeMap::new(),
            settings_rev: presets::SETTINGS_REV,
            cover_spin_s: COVER_SPIN_S,
            cover_pulse_strength: COVER_PULSE,
            home_sections: Vec::new(),
            track_menu: Vec::new(),
            lyrics_keep_misfits: true,
        }
    }
}

impl Prefs {
    /// The prefs of a first start (no prefs file yet): the player as its
    /// owner tuned it, with nothing of theirs (folders, accounts, name),
    /// not starting with Windows, and Главная asks for the name.
    pub fn first_start() -> Self {
        Self {
            fps_limit: 142,
            normalization: false,
            seed_ratio: 3.5,
            // The look of the lyrics is the «Караоке» preset
            // (`presets::Area::Lyrics`): the sung part fills syllable by
            // syllable behind a soft front and floats up a little, the
            // other lines dimmed. The wave's jump keeps its defaults.
            lyrics_style: 0,
            lyrics_unit: LYRICS_BY_SYLLABLES,
            lyrics_light_speed: 0.55,
            lyrics_smooth: 0.6,
            lyrics_dim: 0.7,
            lyrics_glow: true,
            lyrics_cascade: true,
            lyrics_soft_edges: true,
            lyrics_lift: 1.0,
            lyrics_lead: 0.2,
            lyrics_return_s: 2.0,
            lyrics_gap_waves: 7.0,
            lyrics_gap_height: 0.5,
            lyrics_countdown: false,
            lyrics_soft_strength: 0.25,
            lyrics_soft_depth: 0.29,
            save_found_lyrics: true,
            lyrics_deep: false,
            cover_shape: CoverShape::Random,
            ring_style: RingStyle::Circle,
            np_volume_pop: true,
            np_volume_pop_hold: 0.5,
            random_shapes: vec![
                CoverShape::Cookie12,
                CoverShape::Cookie9,
                CoverShape::Cookie7,
                CoverShape::Cookie6,
                CoverShape::Sunny,
                CoverShape::VerySunny,
                CoverShape::Flower,
                CoverShape::Clover4,
                CoverShape::Clover8,
                CoverShape::SoftBurst,
                CoverShape::Puffy,
                CoverShape::PuffyDiamond,
                CoverShape::Pentagon,
                CoverShape::Gem,
            ],
            autostart: false,
            autostart_hidden: false,
            bg_shapes: BgShapes::Morph,
            bg_shapes_count: 10,
            bg_shapes_opacity: 0.25,
            bg_shapes_speed: 0.65,
            bg_shapes_react: 0.5,
            bg_shapes_colors: 1,
            bg_shapes_hue: 6,
            rail_compact: true,
            album_date: AlbumDate::Year,
            spotify_enabled: false,
            library_only_downloaded: true,
            auto_download_limit: 3,
            upgrade_every: UpgradeEvery::Never,
            stream_cache_mb: 700,
            mixes_with_new: true,
            name_prompt: true,
            first_run: true,
            ..Self::default()
        }
    }
}

impl Prefs {
    /// The card at the top of Главная shows: the app has it
    /// ([`HOME_CARD`]) and «Карточка наверху главной» is on.
    pub const fn home_card_shown(&self) -> bool {
        HOME_CARD && self.home_card
    }

    /// The service is switched on.
    pub const fn service_enabled(&self, s: Service) -> bool {
        match s {
            Service::Spotify => self.spotify_enabled,
            Service::Youtube => self.youtube_enabled,
            Service::Deezer => self.deezer_enabled,
            Service::Soundcloud => self.soundcloud_enabled,
        }
    }

    /// Resolves [`Self::language`] against the active system locale.
    pub fn resolved_locale(&self) -> Locale {
        self.language.resolve_system()
    }
}
