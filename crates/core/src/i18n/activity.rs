//! Runtime messages: cold start, home, recs, replica, connect, sync, lyrics, search, streams.

use super::{Locale, PluralForms, common_in, plural_forms};

/// Static strings: runtime messages: cold start, home, recs, replica, connect, sync, lyrics, search, streams.
pub struct ActivityStrings {
    // Cold start: importing a service's history, likes and playlists.
    pub import: &'static str,
    pub import_nothing_found: &'static str,
    pub import_nothing_new: &'static str,
    pub import_nothing: &'static str,
    pub import_likes_here: &'static str,
    pub import_not_music: &'static str,
    pub import_found: &'static str,
    pub import_new: &'static str,
    pub import_not_all: &'static str,
    pub import_failed: &'static str,
    pub import_running: &'static str,
    pub import_started: &'static str,
    pub import_from: &'static str,
    pub import_export: &'static str,
    pub preparing: &'static str,
    pub likes_progress: &'static str,
    pub playlists_progress: &'static str,
    pub playlists: &'static str,
    pub recent_plays: &'static str,
    pub history: &'static str,
    pub history_err: &'static str,
    pub import_err: &'static str,
    pub playlist_err: &'static str,
    pub export_not_found: &'static str,
    pub export_read_failed: &'static str,
    pub playlists_not_opened: &'static str,
    pub playlist_songs: &'static str,
    pub yt_titles: &'static str,
    pub of: &'static str,
    pub plays: PluralForms,
    pub likes: PluralForms,
    pub saved_songs: PluralForms,
    pub playlists_plural: PluralForms,
    pub skipped: PluralForms,

    // Home v2: worlds, their cards and when they were heard.
    pub heard_never: &'static str,
    pub heard_today: &'static str,
    pub heard_yesterday: &'static str,
    pub heard_day_before: &'static str,
    pub heard_prefix: &'static str,
    pub heard_ago: &'static str,
    pub heard_week_ago: &'static str,
    pub heard_two_weeks_ago: &'static str,
    pub heard_long_ago: &'static str,
    pub days: PluralForms,
    pub albums: PluralForms,
    pub releases: PluralForms,
    pub world_finish: &'static str,
    pub world_begin: &'static str,
    pub world_again: &'static str,
    pub card_playing: &'static str,
    pub card_continue: &'static str,
    pub card_from_start: &'static str,
    pub card_in_order: &'static str,
    pub card_never_played: &'static str,
    pub card_favorites: &'static str,
    pub card_newest: &'static str,
    pub listening_now: &'static str,
    pub track_one: &'static str,

    // Recommendations: mixes, moods, daylists and feedback.
    pub and_more: &'static str,
    pub and_last: &'static str,
    pub hero_favorites: &'static str,
    pub hero_listen: &'static str,
    pub by_sound: &'static str,
    pub similar_sound: &'static str,
    pub mood_calm: &'static str,
    pub mood_energetic: &'static str,
    pub mood_dark: &'static str,
    pub mood_bright: &'static str,
    pub mood_focus: &'static str,
    pub mood_drive: &'static str,
    pub daylist_morning: &'static str,
    pub daylist_day: &'static str,
    pub daylist_evening: &'static str,
    pub daylist_night: &'static str,
    pub when_morning: &'static str,
    pub when_day: &'static str,
    pub when_evening: &'static str,
    pub when_night: &'static str,
    pub moment_morning: &'static str,
    pub moment_day: &'static str,
    pub moment_evening: &'static str,
    pub moment_night: &'static str,
    pub new_album: &'static str,
    pub mix: &'static str,
    pub worlds: PluralForms,
    pub unheard: &'static str,
    pub usual: &'static str,
    pub after: &'static str,
    pub from_what_you_play: &'static str,
    pub similar_sound_kicker: &'static str,
    pub sounds_like_your: &'static str,
    pub from_other_worlds: &'static str,
    pub not_recommend_mix: &'static str,
    pub not_recommend_artist: &'static str,
    pub not_recommend_name: &'static str,
    pub not_recommend_artist_q: &'static str,
    pub not_recommend_action: &'static str,
    pub back_in_recs: &'static str,
    pub feedback_err: &'static str,
    pub like_err: &'static str,
    pub in_liked: &'static str,
    pub not_like_q: &'static str,
    pub not_like: &'static str,
    pub you_finish_often: &'static str,
    pub keep_it: &'static str,
    pub keep: &'static str,
    pub at: &'static str,
    pub suggestions: &'static str,
    pub service_no_suggestions: &'static str,
    pub updated: &'static str,
    pub suggestions_sign_in: &'static str,
    pub suggestions_not_loaded: &'static str,
    pub reconnect: &'static str,
    pub to_get_your_top: &'static str,
    pub lottery_subtitle: &'static str,

    // Replica: FLAC masters that may leave this computer.
    pub files: PluralForms,
    pub masters_title: &'static str,
    pub masters_origin: &'static str,
    pub masters_question: &'static str,
    pub masters_skip: &'static str,
    pub masters_accept: &'static str,
    pub untitled: &'static str,
    pub masters_devices_changed: &'static str,
    pub masters_no_answer: &'static str,
    pub masters_choice_changed: &'static str,
    pub masters_unverified: &'static str,
    pub masters_too_few: &'static str,
    pub masters_import_unavailable: &'static str,
    pub masters_stay: &'static str,
    pub masters_stayed: &'static str,
    pub masters_trashed: &'static str,
    pub masters_stayed_count: &'static str,
    pub masters_back: &'static str,
    pub masters_not_back: &'static str,
    pub masters_back_n: &'static str,
    pub masters_not_back_n: &'static str,

    // Quality: fake FLACs offered as MP3s.
    pub quality_busy: &'static str,
    pub separate_tracks: &'static str,
    pub from_lossy: &'static str,
    pub from_mp3: &'static str,
    pub probably: &'static str,
    pub cutoff: &'static str,
    pub fake_flac: &'static str,
    pub keep_flac: &'static str,
    pub fake_question: &'static str,
    pub convert_to_mp3: &'static str,
    pub convert: &'static str,
    pub to_mp3: &'static str,
    pub flac: &'static str,
    pub need_ffmpeg: &'static str,
    pub file_changed: &'static str,
    pub import_unavailable: &'static str,
    pub nothing_to_restore: &'static str,
    pub restored_flac: &'static str,
    pub failed: &'static str,
    pub made_mp3: &'static str,
    pub flac_in_trash: &'static str,
    pub made_mp3_lc: &'static str,
    pub in_trash: &'static str,
    pub could_not_make_mp3: &'static str,

    // Lyrics alignment: the models and what goes wrong.
    pub model_runtime: &'static str,
    pub model_separator: &'static str,
    pub model_aligner_ru: &'static str,
    pub model_aligner_ja: &'static str,
    pub model_aligner_en: &'static str,
    pub model_aligner_mms: &'static str,
    pub model_whisper: &'static str,
    pub model_separator_deep: &'static str,
    pub model_aligner_ru_deep: &'static str,
    pub model_aligner_sung: &'static str,
    pub model_sound_effnet: &'static str,
    pub highlight_failed: &'static str,
    pub models_download_failed: &'static str,
    pub file_unavailable: &'static str,
    pub text_not_alignable: &'static str,
    pub yt_music_off: &'static str,
    pub song_audio_failed: &'static str,

    // Connect: playing on another device.
    pub library_only_on: &'static str,
    pub too_many_moves: &'static str,
    pub no_such_device: &'static str,
    pub offline: &'static str,
    pub mesh_off: &'static str,
    pub move_failed: &'static str,
    pub move_back_failed: &'static str,
    pub device_changed: &'static str,
    pub continue_here: &'static str,
    pub queue_moved: &'static str,
    pub undo_failed: &'static str,
    pub continue_here_failed: &'static str,
    pub no_link: &'static str,

    // Playlist sync.
    pub just_now: &'static str,
    pub mins_ago: &'static str,
    pub hours_ago: &'static str,
    pub days_ago: &'static str,
    pub asks_to_wait: &'static str,
    pub syncing_myself: &'static str,
    pub cannot_change: &'static str,
    pub cannot_change_in: &'static str,
    pub follow_only: &'static str,
    pub sync_no_changes: &'static str,
    pub sync_done: &'static str,
    pub here: &'static str,
    pub liked_syncs_by_likes: &'static str,
    pub sync_two_way: &'static str,
    pub sync_off: &'static str,
    pub sync_label: &'static str,
    pub creating: &'static str,
    pub in_word: &'static str,
    pub playlist_not_linked: &'static str,

    // Lyrics kept by hand.
    pub unnamed_highlight: &'static str,
    pub empty_text: &'static str,
    pub other_text_there: &'static str,
    pub save_text_failed: &'static str,
    pub save_text_unnamed: &'static str,
    pub remove_own_failed: &'static str,
    pub own_in_trash: &'static str,
    pub restore_own_failed: &'static str,
    pub saving_song_yt: &'static str,
    pub save_from_yt: &'static str,
    pub save_song_failed: &'static str,

    // The tag editor.
    pub no_local_file: &'static str,
    pub online_search_off: &'static str,
    pub musicbrainz_no_answer: &'static str,
    pub nothing_found: &'static str,
    pub tags_saved: &'static str,
    pub save_failed: &'static str,
    pub library_files_unavailable: &'static str,
    pub tags_save_failed: &'static str,
    pub could_not: &'static str,

    // The local API.
    pub download_q: &'static str,
    pub dont_download: &'static str,
    pub download: &'static str,
    pub no_settings_place: &'static str,
    pub settings_unreadable: &'static str,
    pub token_unnamed: &'static str,
    pub token_exists: &'static str,
    pub token: &'static str,
    pub revoked: &'static str,
    pub playlist: &'static str,
    pub saved: &'static str,
    pub dont_know_what: &'static str,
    pub no_download_sources: &'static str,

    // Why a stream did not play.
    pub yt_age_signed_in: &'static str,
    pub yt_age_sign_in: &'static str,
    pub sc_drm: &'static str,
    pub region_blocked: &'static str,
    pub the_service: &'static str,
    pub yt_refusing_audio: &'static str,
    pub could_not_play: &'static str,

    // Sharing a link.
    pub share_searching: &'static str,
    pub link_copied: &'static str,
    pub musiclink_key_refused_songlink: &'static str,
    pub musiclink_no_answer: &'static str,
    pub musiclink_key_refused: &'static str,
    pub deezer_no_answer: &'static str,
    pub songlink_no_answer: &'static str,
    pub not_in_services: &'static str,

    // Likes pulled from a service.
    pub likes_err: &'static str,
    pub likes_plural: PluralForms,
    pub waiting_download: &'static str,

    // Signing in through a window.
    pub window_hint: &'static str,
    pub soundcloud_window_hint: &'static str,
    pub sc_refused: &'static str,
    pub login_closed: &'static str,
    pub login_timed_out: &'static str,
    pub login_refused: &'static str,
    pub login_cancelled: &'static str,

    // Replacing a track with a stream's copy.
    pub yt_deezer_off: &'static str,
    pub need_yt_sign_in: &'static str,
    pub clip_not_liked_yt: &'static str,
    pub track_not_liked_yt: &'static str,
    pub replace_err: &'static str,
    pub undo_replace_err: &'static str,
    pub only_here_no_yt: &'static str,
    pub replace_undone: &'static str,

    // A service that is slow, overloaded or unreachable in search.
    pub search_overloaded: &'static str,
    pub search_no_connection: &'static str,

    // Prowlarr and the torrent search.
    pub prowlarr_not_set_up_sources: &'static str,
    pub prowlarr_no_answer_90: &'static str,
    pub prowlarr_not_set_up: &'static str,
    pub checking: &'static str,
    pub enter_prowlarr_key: &'static str,
    pub prowlarr_bad_address: &'static str,
    pub no_torrents_found: &'static str,
    pub prowlarr_no_trackers: &'static str,
    pub prowlarr_answers: &'static str,
    pub trackers_plural: PluralForms,
    pub prowlarr_key_refused: &'static str,
    pub prowlarr_unreachable: &'static str,
    pub not_prowlarr: &'static str,

    // Refreshing a cover.
    pub cover_looking: &'static str,
    pub no_better_cover: &'static str,
    pub cover_refresh_failed: &'static str,

    // The same audio on two files.
    pub same_song_q: &'static str,
    pub different_songs: &'static str,
    pub one_song: &'static str,
    pub which_file_other: &'static str,
    pub all_correct: &'static str,
    pub file_has_other: &'static str,
    pub put_back: &'static str,
    pub file_back: &'static str,
    pub file_not_back: &'static str,
    pub could_not_restore_file: &'static str,

    // What a failed library read was about.
    pub lib_library: &'static str,
    pub lib_artists: &'static str,
    pub lib_tracks: &'static str,

    // Albums that look like editions of one another.
    pub editions_repeat: &'static str,
    pub editions_merge_failed: &'static str,

    // Mesh.
    pub mesh_not_running: &'static str,

    // A service's playlist into the library.
    pub playlist_already_in_library: &'static str,
    pub playlist_in_library: &'static str,
}

pub static RU: ActivityStrings = ActivityStrings {
    import: "Импорт",
    import_nothing_found: "Импорт: ничего не нашлось",
    import_nothing_new: "Импорт: ничего нового — всё уже было",
    import_nothing: "ничего",
    import_likes_here: "лайки уже здесь",
    import_not_music: "не с музыкой",
    import_found: "Найдено",
    import_new: "новых",
    import_not_all: "не всё",
    import_failed: "Импорт не удался",
    import_running: "Этот импорт уже идёт · смотри «Загрузки»",
    import_started: "Импорт начался · смотри «Загрузки»",
    import_from: "Импорт из",
    import_export: "Импорт выгрузки",
    preparing: "Подготовка…",
    likes_progress: "Лайки…",
    playlists_progress: "Плейлисты…",
    playlists: "Плейлисты",
    recent_plays: "Недавние прослушивания…",
    history: "История",
    history_err: "история",
    import_err: "импорт",
    playlist_err: "плейлист",
    export_not_found: "Не нашёл файлов выгрузки Spotify или Google Takeout",
    export_read_failed: "Не получилось прочитать",
    playlists_not_opened: "не открылись плейлисты",
    playlist_songs: "Ищу песни плейлистов",
    yt_titles: "Узнаю названия в YouTube Music",
    of: "из",
    plays: PluralForms { one: "прослушивание", few: "прослушивания", many: "прослушиваний" },
    likes: PluralForms { one: "лайк", few: "лайка", many: "лайков" },
    saved_songs: PluralForms { one: "песня медиатеки", few: "песни медиатеки", many: "песен медиатеки" },
    playlists_plural: PluralForms { one: "плейлист", few: "плейлиста", many: "плейлистов" },
    skipped: PluralForms { one: "пропущен", few: "пропущено", many: "пропущено" },

    heard_never: "Ещё не начинал",
    heard_today: "Слушал сегодня",
    heard_yesterday: "Слушал вчера",
    heard_day_before: "Слушал позавчера",
    heard_prefix: "Слушал",
    heard_ago: "назад",
    heard_week_ago: "Слушал неделю назад",
    heard_two_weeks_ago: "Слушал две недели назад",
    heard_long_ago: "Давно не слушал",
    days: PluralForms { one: "день", few: "дня", many: "дней" },
    albums: PluralForms { one: "альбом", few: "альбома", many: "альбомов" },
    releases: PluralForms { one: "релиз", few: "релиза", many: "релизов" },
    world_finish: "что ещё не слышал, по порядку альбомов",
    world_begin: "с первого трека первого альбома",
    world_again: "всё с первого трека",
    card_playing: "Сейчас играет",
    card_continue: "Продолжить",
    card_from_start: "С начала",
    card_in_order: "По порядку",
    card_never_played: "Ни разу не слушал",
    card_favorites: "Любимое",
    card_newest: "Новее всего",
    listening_now: "Слушаешь сейчас",
    track_one: "трек",

    and_more: "и другие",
    and_last: "и",
    hero_favorites: "любимое и похожее",
    hero_listen: "Слушай — микс соберётся из того, что нравится",
    by_sound: "по звуку",
    similar_sound: "похожего звука",
    mood_calm: "тихое и неспешное",
    mood_energetic: "быстрое и громкое",
    mood_dark: "минорное, тёмное по звуку",
    mood_bright: "мажорное, светлое по звуку",
    mood_focus: "ровное и негромкое",
    mood_drive: "громкое и ровное, в дорогу",
    daylist_morning: "НА УТРО",
    daylist_day: "НА ДЕНЬ",
    daylist_evening: "НА ВЕЧЕР",
    daylist_night: "НА НОЧЬ",
    when_morning: "по утрам",
    when_day: "днём",
    when_evening: "по вечерам",
    when_night: "по ночам",
    moment_morning: "Микс на утро",
    moment_day: "Микс на день",
    moment_evening: "Микс на вечер",
    moment_night: "Микс на ночь",
    new_album: "Новый альбом",
    mix: "Микс",
    worlds: PluralForms { one: "мир", few: "мира", many: "миров" },
    unheard: "ты ещё не слышал",
    usual: "Как обычно",
    after: "После",
    from_what_you_play: "Из того, что ты слушаешь",
    similar_sound_kicker: "Похоже по звуку",
    sounds_like_your: "Звучит как твой",
    from_other_worlds: "но из других миров",
    not_recommend_mix: "Не буду предлагать",
    not_recommend_artist: "Не буду рекомендовать исполнителя",
    not_recommend_name: "Не буду рекомендовать",
    not_recommend_artist_q: "Не рекомендовать исполнителя",
    not_recommend_action: "Не рекомендовать",
    back_in_recs: "снова в рекомендациях",
    feedback_err: "отзыв",
    like_err: "лайк",
    in_liked: "в Любимых",
    not_like_q: "Не нравится?",
    not_like: "Не нравится",
    you_finish_often: "ты часто дослушиваешь",
    keep_it: "Оставить у себя",
    keep: "Оставить",
    at: "в",
    suggestions: "Подсказки",
    service_no_suggestions: "сервис пока ничего не советует",
    updated: "обновлено",
    suggestions_sign_in: "Подсказки — после входа в аккаунт",
    suggestions_not_loaded: "Подсказки ещё не загружены",
    reconnect: "Переподключи",
    to_get_your_top: "чтобы брать твой топ",
    lottery_subtitle: "каждый раз другое — жми и узнаешь",

    files: PluralForms { one: "файл", few: "файла", many: "файлов" },
    masters_title: "Оригиналы FLAC",
    masters_origin: "Копии есть на других устройствах",
    masters_question: "Убрать их с компьютера в Корзину? Копии на устройствах останутся",
    masters_skip: "Оставить",
    masters_accept: "Убрать в Корзину",
    untitled: "Без названия",
    masters_devices_changed: "Устройства или выбор изменились",
    masters_no_answer: "Устройства не ответили",
    masters_choice_changed: "Выбор изменился",
    masters_unverified: "Копии не проверены",
    masters_too_few: "Копий меньше нужного",
    masters_import_unavailable: "Импорт недоступен",
    masters_stay: "оригиналы остались на компьютере",
    masters_stayed: "Оригиналы остались на компьютере",
    masters_trashed: "Оригиналы FLAC в Корзине",
    masters_stayed_count: "остались на компьютере",
    masters_back: "Оригиналы вернулись",
    masters_not_back: "Оригиналы не вернулись: их место занято или их нет в Корзине",
    masters_back_n: "Вернулись",
    masters_not_back_n: "не вернулись",

    quality_busy: "Ещё не закончил с прошлым файлом",
    separate_tracks: "Отдельные треки",
    from_lossy: "из сжатого",
    from_mp3: "из MP3",
    probably: "Вероятно",
    cutoff: "срез",
    fake_flac: "Ненастоящий FLAC",
    keep_flac: "Оставить FLAC",
    fake_question: "Отметь, какие превратить в MP3: FLAC уйдут в Корзину, «Вернуть FLAC» в настройках вернёт их",
    convert_to_mp3: "Превратить в MP3",
    convert: "Превратить",
    to_mp3: "в MP3",
    flac: "FLAC",
    need_ffmpeg: "Нужен ffmpeg, чтобы сделать MP3",
    file_changed: "Файл изменился после проверки — проверю его заново",
    import_unavailable: "Импорт недоступен",
    nothing_to_restore: "Возвращать нечего: все FLAC на месте",
    restored_flac: "Вернул FLAC",
    failed: "не получилось",
    made_mp3: "Сделал MP3",
    flac_in_trash: "FLAC в Корзине",
    made_mp3_lc: "сделал MP3",
    in_trash: "в Корзине",
    could_not_make_mp3: "Не получилось сделать MP3 из",

    model_runtime: "Основа для моделей (ONNX Runtime)",
    model_separator: "Отделение голоса от музыки",
    model_aligner_ru: "Слова по голосу: русский",
    model_aligner_ja: "Слова по голосу: японский",
    model_aligner_en: "Слова по голосу: английский",
    model_aligner_mms: "Слова по голосу: другие языки",
    model_whisper: "Расслышать текст",
    model_separator_deep: "Глубокое создание: отделение голоса",
    model_aligner_ru_deep: "Глубокое создание: русский",
    model_aligner_sung: "Глубокое создание: английский и другие языки",
    model_sound_effnet: "Нейросетевой анализ звука",
    highlight_failed: "Не получилось сделать подсветку",
    models_download_failed: "Не удалось скачать модели",
    file_unavailable: "Файл недоступен",
    text_not_alignable: "Этот текст не выровнять",
    yt_music_off: "YouTube Music выключен",
    song_audio_failed: "Не удалось получить звук песни",

    library_only_on: "играют только песни из библиотеки",
    too_many_moves: "Слишком много переносов подряд",
    no_such_device: "Такого устройства нет",
    offline: "не в сети",
    mesh_off: "Межсеть не запущена",
    move_failed: "Не удалось перенести на",
    move_back_failed: "Не удалось вернуть музыку сюда",
    device_changed: "Устройство сменилось",
    continue_here: "Продолжить здесь",
    queue_moved: "Очередь перенесена на",
    undo_failed: "Не удалось отменить",
    continue_here_failed: "Не удалось продолжить здесь",
    no_link: "Нет связи с",

    just_now: "только что",
    mins_ago: "мин назад",
    hours_ago: "ч назад",
    days_ago: "дн. назад",
    asks_to_wait: "просит подождать",
    syncing_myself: "синхронизирую сам",
    cannot_change: "менять нельзя",
    cannot_change_in: "менять нельзя в",
    follow_only: "только получать изменения",
    sync_no_changes: "Плейлист синхронизирован: изменений нет",
    sync_done: "Плейлист синхронизирован",
    here: "здесь",
    liked_syncs_by_likes: "«Любимые» синхронизируются лайками",
    sync_two_way: "изменения в обе стороны",
    sync_off: "выключена",
    sync_label: "Синхронизация",
    creating: "Создаю",
    in_word: "в",
    playlist_not_linked: "Плейлист не связан с сервисом",

    unnamed_highlight: "Подсветку не сделать: неясно, чья это песня",
    empty_text: "пустой текст",
    other_text_there: "на его месте уже другой текст",
    save_text_failed: "Не получилось сохранить текст",
    save_text_unnamed: "Не получилось сохранить текст: неясно, чья это песня",
    remove_own_failed: "Не получилось убрать свой текст",
    own_in_trash: "Свой текст в корзине",
    restore_own_failed: "Не получилось вернуть свой текст",
    saving_song_yt: "Сохраняю песню с YouTube…",
    save_from_yt: "Сохранение с YouTube",
    save_song_failed: "Не получилось сохранить песню",

    no_local_file: "У трека нет локального файла",
    online_search_off: "Онлайн-поиск выключен",
    musicbrainz_no_answer: "MusicBrainz не ответил",
    nothing_found: "Ничего не нашлось",
    tags_saved: "Теги сохранены",
    save_failed: "Не удалось сохранить",
    library_files_unavailable: "Файлы библиотеки недоступны",
    tags_save_failed: "Не удалось сохранить теги",
    could_not: "не удалось",

    download_q: "Скачать?",
    dont_download: "Не скачивать",
    download: "Скачать",
    no_settings_place: "Некуда сохранить настройки",
    settings_unreadable: "settings.toml не читается — поправь файл вручную",
    token_unnamed: "Токен без названия",
    token_exists: "Такой токен уже есть",
    token: "Токен",
    revoked: "отозван",
    playlist: "Плейлист",
    saved: "сохранён",
    dont_know_what: "Не знаю, что искать: у песни нет исполнителя или названия",
    no_download_sources: "Источники загрузок не настроены",

    yt_age_signed_in: "на YouTube только для 18+, и со входом не открылся",
    yt_age_sign_in: "на YouTube только для 18+ — нужен вход в YouTube Music",
    sc_drm: "в SoundCloud только в защищённом виде",
    region_blocked: "недоступно в твоём регионе",
    the_service: "сервис",
    yt_refusing_audio: "YouTube сейчас не отдаёт звук — попробуй чуть позже",
    could_not_play: "не получилось воспроизвести",

    share_searching: "Ищу ссылку…",
    link_copied: "Ссылка скопирована",
    musiclink_key_refused_songlink: "Ключ MusicLink не подошёл — скопирована ссылка song.link",
    musiclink_no_answer: "MusicLink не ответил",
    musiclink_key_refused: "Ключ MusicLink не подошёл",
    deezer_no_answer: "Deezer не ответил",
    songlink_no_answer: "song.link не ответил",
    not_in_services: "Нет в сервисах — скопировано название",

    likes_err: "лайки",
    likes_plural: PluralForms { one: "лайк", few: "лайка", many: "лайков" },
    waiting_download: " · ждут скачивания",

    window_hint: "Войди в аккаунт Google в открывшемся окне. Пароль вводится только на странице Google — приложение его не видит.",
    soundcloud_window_hint: "Войди в SoundCloud в открывшемся окне. Пароль вводится только на странице SoundCloud — приложение его не видит.",
    sc_refused: "SoundCloud не разрешил вход во встроенном окне. Можно вставить токен: «Вставить токен…»",
    login_closed: "Вход отменён: окно закрыто раньше, чем вход завершился",
    login_timed_out: "Вход не завершился за 10 минут — попробуй ещё раз",
    login_refused: "Google не разрешил вход во встроенном окне. Войди по cookies: «Вставить cookies…»",
    login_cancelled: "Вход отменён",

    yt_deezer_off: "YouTube Music и Deezer выключены",
    need_yt_sign_in: "Нужен вход в YouTube Music",
    clip_not_liked_yt: "Клип не лайкнут в YouTube Music",
    track_not_liked_yt: "Трек не лайкнут в YouTube Music",
    replace_err: "замена трека",
    undo_replace_err: "отмена замены",
    only_here_no_yt: " · только здесь: в YouTube Music нет этой песни",
    replace_undone: "Замена отменена",

    search_overloaded: "сейчас перегружен",
    search_no_connection: "Нет связи с",

    prowlarr_not_set_up_sources: "Prowlarr не настроен — адрес и ключ в Настройках → Источники → Торренты",
    prowlarr_no_answer_90: "Prowlarr не ответил за 90 с",
    prowlarr_not_set_up: "Prowlarr не настроен",
    checking: "Проверяю…",
    enter_prowlarr_key: "Введи API-ключ Prowlarr",
    prowlarr_bad_address: "Адрес Prowlarr не подходит — нужен http:// или https://",
    no_torrents_found: "Раздач не нашлось",
    prowlarr_no_trackers: "Prowlarr отвечает, но трекеры в нём не включены",
    prowlarr_answers: "Prowlarr отвечает",
    trackers_plural: PluralForms { one: "трекер", few: "трекера", many: "трекеров" },
    prowlarr_key_refused: "Prowlarr не принял ключ — проверь API-ключ",
    prowlarr_unreachable: "Prowlarr не отвечает по этому адресу",
    not_prowlarr: "По этому адресу отвечает не Prowlarr",

    cover_looking: "Ищу обложку…",
    no_better_cover: "Лучшей обложки не нашлось",
    cover_refresh_failed: "Не получилось обновить обложку",

    same_song_q: "Одна и та же песня?",
    different_songs: "Разные песни",
    one_song: "Одна песня",
    which_file_other: "В каком файле другая песня?",
    all_correct: "Всё верно",
    file_has_other: "В файле другая песня",
    put_back: "Вернуть",
    file_back: "Файл вернулся",
    file_not_back: "Файл не вернулся: его место занято",
    could_not_restore_file: "Не получилось вернуть файл",

    lib_library: "библиотека",
    lib_artists: "исполнители",
    lib_tracks: "треки",

    editions_repeat: "Похоже, некоторые альбомы повторяются — посмотри в Библиотеке",
    editions_merge_failed: "Не получилось объединить альбомы",

    mesh_not_running: "Межсеть не запущена",

    playlist_already_in_library: "уже в библиотеке",
    playlist_in_library: "в библиотеке",
};
pub static EN: ActivityStrings = ActivityStrings {
    import: "Import",
    import_nothing_found: "Import: nothing found",
    import_nothing_new: "Import: nothing new — it was all here",
    import_nothing: "nothing",
    import_likes_here: "likes already here",
    import_not_music: "not music",
    import_found: "Found",
    import_new: "new",
    import_not_all: "not everything",
    import_failed: "Import failed",
    import_running: "This import is already running · see Downloads",
    import_started: "Import started · see Downloads",
    import_from: "Import from",
    import_export: "Import of",
    preparing: "Preparing…",
    likes_progress: "Likes…",
    playlists_progress: "Playlists…",
    playlists: "Playlists",
    recent_plays: "Recent plays…",
    history: "History",
    history_err: "history",
    import_err: "import",
    playlist_err: "playlist",
    export_not_found: "No Spotify or Google Takeout export files found",
    export_read_failed: "Could not read",
    playlists_not_opened: "playlists could not be opened",
    playlist_songs: "Looking up playlist songs",
    yt_titles: "Looking up titles in YouTube Music",
    of: "of",
    plays: PluralForms { one: "play", few: "plays", many: "plays" },
    likes: PluralForms { one: "like", few: "likes", many: "likes" },
    saved_songs: PluralForms { one: "library song", few: "library songs", many: "library songs" },
    playlists_plural: PluralForms { one: "playlist", few: "playlists", many: "playlists" },
    skipped: PluralForms { one: "skipped", few: "skipped", many: "skipped" },

    heard_never: "Not started yet",
    heard_today: "Listened today",
    heard_yesterday: "Listened yesterday",
    heard_day_before: "Listened 2 days ago",
    heard_prefix: "Listened",
    heard_ago: "ago",
    heard_week_ago: "Listened a week ago",
    heard_two_weeks_ago: "Listened two weeks ago",
    heard_long_ago: "Not listened in a while",
    days: PluralForms { one: "day", few: "days", many: "days" },
    albums: PluralForms { one: "album", few: "albums", many: "albums" },
    releases: PluralForms { one: "release", few: "releases", many: "releases" },
    world_finish: "what you haven't heard, in album order",
    world_begin: "from the first track of the first album",
    world_again: "everything from the first track",
    card_playing: "Playing now",
    card_continue: "Continue",
    card_from_start: "From the start",
    card_in_order: "In order",
    card_never_played: "Never played",
    card_favorites: "Favorites",
    card_newest: "Newest",
    listening_now: "Listening now",
    track_one: "track",

    and_more: "& more",
    and_last: "&",
    hero_favorites: "favorites and similar",
    hero_listen: "Listen — your mix will form from what you enjoy",
    by_sound: "sound",
    similar_sound: "with a similar sound",
    mood_calm: "quiet and unhurried",
    mood_energetic: "fast and loud",
    mood_dark: "minor, dark in tone",
    mood_bright: "major, bright in tone",
    mood_focus: "steady and gentle",
    mood_drive: "loud and steady, for the road",
    daylist_morning: "MORNING",
    daylist_day: "DAYTIME",
    daylist_evening: "EVENING",
    daylist_night: "NIGHT",
    when_morning: "in the mornings",
    when_day: "during the day",
    when_evening: "in the evenings",
    when_night: "at night",
    moment_morning: "Morning Mix",
    moment_day: "Daytime Mix",
    moment_evening: "Evening Mix",
    moment_night: "Night Mix",
    new_album: "New Album",
    mix: "Mix",
    worlds: PluralForms { one: "world", few: "worlds", many: "worlds" },
    unheard: "unheard",
    usual: "Usual",
    after: "After",
    from_what_you_play: "From what you play",
    similar_sound_kicker: "Similar Sound",
    sounds_like_your: "Sounds like your",
    from_other_worlds: "from other worlds",
    not_recommend_mix: "Won't recommend",
    not_recommend_artist: "Won't recommend artist",
    not_recommend_name: "Won't recommend",
    not_recommend_artist_q: "Don't recommend artist",
    not_recommend_action: "Don't recommend",
    back_in_recs: "is back in recommendations",
    feedback_err: "feedback",
    like_err: "like",
    in_liked: "is in Liked Songs",
    not_like_q: "Not a fan?",
    not_like: "Not a fan",
    you_finish_often: "you often listen to it through",
    keep_it: "Keep",
    keep: "Keep",
    at: "at",
    suggestions: "Suggestions",
    service_no_suggestions: "the service has nothing to suggest yet",
    updated: "updated",
    suggestions_sign_in: "Suggestions — after you sign in",
    suggestions_not_loaded: "Suggestions not loaded yet",
    reconnect: "Reconnect",
    to_get_your_top: "to get your top",
    lottery_subtitle: "different every time — tap to find out",

    files: PluralForms { one: "file", few: "files", many: "files" },
    masters_title: "FLAC masters",
    masters_origin: "Copies are on other devices",
    masters_question: "Move them from this computer to the Trash? The copies on your devices stay",
    masters_skip: "Keep",
    masters_accept: "Move to Trash",
    untitled: "Untitled",
    masters_devices_changed: "Devices or the selection changed",
    masters_no_answer: "Devices did not answer",
    masters_choice_changed: "The selection changed",
    masters_unverified: "Copies are not verified",
    masters_too_few: "Fewer copies than needed",
    masters_import_unavailable: "Import is unavailable",
    masters_stay: "the masters stay on this computer",
    masters_stayed: "The masters stay on this computer",
    masters_trashed: "FLAC masters in the Trash",
    masters_stayed_count: "stayed on this computer",
    masters_back: "The masters are back",
    masters_not_back: "The masters did not come back: their place is taken or they are not in the Trash",
    masters_back_n: "Back",
    masters_not_back_n: "did not come back",

    quality_busy: "Still finishing the previous file",
    separate_tracks: "Separate tracks",
    from_lossy: "from a lossy source",
    from_mp3: "from MP3",
    probably: "Probably",
    cutoff: "cutoff",
    fake_flac: "Fake FLAC",
    keep_flac: "Keep FLAC",
    fake_question: "Tick the ones to turn into MP3: the FLAC files go to the Trash, “Restore FLAC” in Settings brings them back",
    convert_to_mp3: "Convert to MP3",
    convert: "Convert",
    to_mp3: "to MP3",
    flac: "FLAC",
    need_ffmpeg: "ffmpeg is needed to make MP3",
    file_changed: "The file changed after the check — I'll check it again",
    import_unavailable: "Import is unavailable",
    nothing_to_restore: "Nothing to restore: all the FLAC files are in place",
    restored_flac: "Restored FLAC",
    failed: "failed",
    made_mp3: "Made MP3",
    flac_in_trash: "FLAC in the Trash",
    made_mp3_lc: "made MP3",
    in_trash: "in the Trash",
    could_not_make_mp3: "Could not make MP3 from",

    model_runtime: "Model base (ONNX Runtime)",
    model_separator: "Voice from music separation",
    model_aligner_ru: "Words from the voice: Russian",
    model_aligner_ja: "Words from the voice: Japanese",
    model_aligner_en: "Words from the voice: English",
    model_aligner_mms: "Words from the voice: other languages",
    model_whisper: "Make out the lyrics",
    model_separator_deep: "Deep creation: voice separation",
    model_aligner_ru_deep: "Deep creation: Russian",
    model_aligner_sung: "Deep creation: English and other languages",
    model_sound_effnet: "Neural sound analysis",
    highlight_failed: "Could not make the highlight",
    models_download_failed: "Could not download the models",
    file_unavailable: "File is unavailable",
    text_not_alignable: "This text cannot be aligned",
    yt_music_off: "YouTube Music is off",
    song_audio_failed: "Could not get the song's audio",

    library_only_on: "plays only library songs",
    too_many_moves: "Too many moves in a row",
    no_such_device: "No such device",
    offline: "is offline",
    mesh_off: "Mesh is not running",
    move_failed: "Could not move to",
    move_back_failed: "Could not bring the music back here",
    device_changed: "The device changed",
    continue_here: "Continue here",
    queue_moved: "Queue moved to",
    undo_failed: "Could not undo",
    continue_here_failed: "Could not continue here",
    no_link: "No connection to",

    just_now: "just now",
    mins_ago: "min ago",
    hours_ago: "hr ago",
    days_ago: "days ago",
    asks_to_wait: "asks to wait",
    syncing_myself: "syncing myself",
    cannot_change: "can't be changed",
    cannot_change_in: "can't be changed in",
    follow_only: "follow only",
    sync_no_changes: "Playlist synced: no changes",
    sync_done: "Playlist synced",
    here: "here",
    liked_syncs_by_likes: "“Liked Songs” syncs through likes",
    sync_two_way: "changes both ways",
    sync_off: "off",
    sync_label: "Sync",
    creating: "Creating",
    in_word: "in",
    playlist_not_linked: "The playlist is not linked to a service",

    unnamed_highlight: "Can't make the highlight: it's unclear whose song this is",
    empty_text: "empty text",
    other_text_there: "another text is in its place",
    save_text_failed: "Could not save the lyrics",
    save_text_unnamed: "Could not save the lyrics: it's unclear whose song this is",
    remove_own_failed: "Could not remove your lyrics",
    own_in_trash: "Your lyrics are in the Trash",
    restore_own_failed: "Could not bring your lyrics back",
    saving_song_yt: "Saving the song from YouTube…",
    save_from_yt: "Saving from YouTube",
    save_song_failed: "Could not save the song",

    no_local_file: "The track has no local file",
    online_search_off: "Online search is off",
    musicbrainz_no_answer: "MusicBrainz did not answer",
    nothing_found: "Nothing found",
    tags_saved: "Tags saved",
    save_failed: "Could not save",
    library_files_unavailable: "Library files are unavailable",
    tags_save_failed: "Could not save the tags",
    could_not: "failed",

    download_q: "Download?",
    dont_download: "Don't download",
    download: "Download",
    no_settings_place: "Nowhere to save the settings",
    settings_unreadable: "settings.toml cannot be read — fix the file by hand",
    token_unnamed: "Token without a name",
    token_exists: "That token already exists",
    token: "Token",
    revoked: "revoked",
    playlist: "Playlist",
    saved: "saved",
    dont_know_what: "Nothing to search for: the song has no artist or title",
    no_download_sources: "Download sources are not set up",

    yt_age_signed_in: "YouTube marked this 18+, and it did not open even signed in",
    yt_age_sign_in: "YouTube marked this 18+ — sign in to YouTube Music",
    sc_drm: "only DRM-protected on SoundCloud",
    region_blocked: "unavailable in your region",
    the_service: "the service",
    yt_refusing_audio: "YouTube is refusing audio right now — try again later",
    could_not_play: "could not play",

    share_searching: "Looking for a link…",
    link_copied: "Link copied",
    musiclink_key_refused_songlink: "MusicLink key refused — copied the song.link",
    musiclink_no_answer: "MusicLink did not answer",
    musiclink_key_refused: "MusicLink key refused",
    deezer_no_answer: "Deezer did not answer",
    songlink_no_answer: "song.link did not answer",
    not_in_services: "Not on any service — copied the title",

    likes_err: "likes",
    likes_plural: PluralForms { one: "like", few: "likes", many: "likes" },
    waiting_download: " · waiting to download",

    window_hint: "Sign in to your Google account in the window that opened. The password is typed only on Google's page — the app never sees it.",
    soundcloud_window_hint: "Sign in to SoundCloud in the window that opened. The password is typed only on SoundCloud's page — the app never sees it.",
    sc_refused: "SoundCloud refused the sign-in in the embedded window. You can paste a token: “Paste token…”",
    login_closed: "Sign-in cancelled: the window closed before it finished",
    login_timed_out: "Sign-in did not finish in 10 minutes — try again",
    login_refused: "Google refused the sign-in in the embedded window. Sign in with cookies: “Paste cookies…”",
    login_cancelled: "Sign-in cancelled",

    yt_deezer_off: "YouTube Music and Deezer are off",
    need_yt_sign_in: "Sign in to YouTube Music",
    clip_not_liked_yt: "The clip is not liked on YouTube Music",
    track_not_liked_yt: "The track is not liked on YouTube Music",
    replace_err: "track replacement",
    undo_replace_err: "replacement undo",
    only_here_no_yt: " · here only: YouTube Music does not have this song",
    replace_undone: "Replacement undone",

    search_overloaded: "is overloaded right now",
    search_no_connection: "Cannot reach",

    prowlarr_not_set_up_sources: "Prowlarr is not set up — the address and key are in Settings → Sources → Torrents",
    prowlarr_no_answer_90: "Prowlarr did not answer in 90 s",
    prowlarr_not_set_up: "Prowlarr is not set up",
    checking: "Checking…",
    enter_prowlarr_key: "Enter the Prowlarr API key",
    prowlarr_bad_address: "That Prowlarr address does not work — it needs http:// or https://",
    no_torrents_found: "No torrents found",
    prowlarr_no_trackers: "Prowlarr answers, but no trackers are enabled in it",
    prowlarr_answers: "Prowlarr answers",
    trackers_plural: PluralForms { one: "tracker", few: "trackers", many: "trackers" },
    prowlarr_key_refused: "Prowlarr did not accept the key — check the API key",
    prowlarr_unreachable: "Prowlarr does not answer at that address",
    not_prowlarr: "That address is not Prowlarr",

    cover_looking: "Looking for a cover…",
    no_better_cover: "No better cover found",
    cover_refresh_failed: "Could not refresh the cover",

    same_song_q: "The same song?",
    different_songs: "Different songs",
    one_song: "One song",
    which_file_other: "Which file has the other song?",
    all_correct: "All correct",
    file_has_other: "The file has the other song",
    put_back: "Put back",
    file_back: "The file is back",
    file_not_back: "The file did not come back: its place is taken",
    could_not_restore_file: "Could not put the file back",

    lib_library: "library",
    lib_artists: "artists",
    lib_tracks: "tracks",

    editions_repeat: "Some albums look like duplicates — check the Library",
    editions_merge_failed: "Could not merge the albums",

    mesh_not_running: "Mesh is not running",

    playlist_already_in_library: "is already in the library",
    playlist_in_library: "is in the library",
};

locale_table! {
    /// Returns these strings for `loc`.
    activity_in -> ActivityStrings,
    /// Returns these strings for the active process locale.
    activity,
    { Ru => RU, En => EN }
}

/// `1240` → `"1 240"` (a no-break space) in Russian, `"1,240"` in English.
pub fn group_in(loc: Locale, n: usize) -> String {
    let sep = match loc {
        Locale::Ru => '\u{a0}',
        Locale::En => ',',
    };
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Wraps `name` in the locale's quotes: `«X»` in Russian, `“X”` in English.
pub fn fmt_quoted_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("«{name}»"),
        Locale::En => format!("“{name}”"),
    }
}

/// `"3 прослушивания, 2 плейлиста"` / `"3 plays, 2 playlists"`: the counts that are not zero.
pub fn fmt_import_counts_in(loc: Locale, plays: usize, likes: usize, saved: usize, playlists: usize) -> String {
    let s = activity_in(loc);
    let parts: Vec<String> = [
        (plays, &s.plays),
        (likes, &s.likes),
        (saved, &s.saved_songs),
        (playlists, &s.playlists_plural),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, forms)| format!("{} {}", group_in(loc, n), plural_forms(loc, n as u64, forms)))
    .collect();
    if parts.is_empty() { s.import_nothing.to_owned() } else { parts.join(", ") }
}

/// `" · пропущено 2 плейлиста не с музыкой"` / `" · 2 playlists skipped, not music"`, or nothing.
pub fn fmt_import_skipped_in(loc: Locale, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let s = activity_in(loc);
    let count = group_in(loc, n);
    let list = plural_forms(loc, n as u64, &s.playlists_plural);
    let verb = plural_forms(loc, n as u64, &s.skipped);
    match loc {
        Locale::Ru => format!(" · {verb} {count} {list} {}", s.import_not_music),
        Locale::En => format!(" · {count} {list} {verb}, {}", s.import_not_music),
    }
}

/// `"52 мин"` / `"1 ч 05 мин"`: minutes rounded, the rest zero-padded.
pub fn fmt_minutes_in(loc: Locale, ms: u64) -> String {
    let s = common_in(loc);
    let m = (ms + 30_000) / 60_000;
    if m < 60 {
        format!("{m} {}", s.min_short)
    } else {
        format!("{} {} {:02} {}", m / 60, s.hr_short, m % 60, s.min_short)
    }
}

/// `"Переподключи Spotify, чтобы брать твой топ"` / `"Reconnect Spotify to get your top"`.
pub fn fmt_reconnect_in(loc: Locale, service: &str) -> String {
    let s = activity_in(loc);
    match loc {
        Locale::Ru => format!("{} {service}, {}", s.reconnect, s.to_get_your_top),
        Locale::En => format!("{} {service} {}", s.reconnect, s.to_get_your_top),
    }
}

/// `"820 МБ"` / `"1,4 ГБ"` (English: `"820 MB"` / `"1.4 GB"`).
pub fn fmt_size_in(loc: Locale, bytes: u64) -> String {
    const MB: f64 = (1u64 << 20) as f64;
    const GB: f64 = (1u64 << 30) as f64;
    let s = common_in(loc);
    let b = bytes as f64;
    if b >= GB {
        let v = format!("{:.1}", b / GB);
        match loc {
            Locale::Ru => format!("{} {}", v.replace('.', ","), s.gb),
            Locale::En => format!("{} {}", v, s.gb),
        }
    } else {
        format!("{} {}", (b / MB).round().max(1.0) as u64, s.mb)
    }
}

/// `"На Raspberry Pi играют только песни из библиотеки"` / `"Raspberry Pi plays only library songs"`.
pub fn fmt_library_only_in(loc: Locale, name: &str) -> String {
    let s = activity_in(loc);
    match loc {
        Locale::Ru => format!("На {name} {}", s.library_only_on),
        Locale::En => format!("{name} {}", s.library_only_on),
    }
}

/// `"«X» в Spotify менять нельзя — только получать изменения"` /
/// `"“X” can't be changed in Spotify — follow only"`.
pub fn fmt_cannot_change_in(loc: Locale, name: &str, service: &str) -> String {
    let s = activity_in(loc);
    let quoted = fmt_quoted_in(loc, name);
    match loc {
        Locale::Ru => format!("{quoted} {} {service} {} — {}", s.in_word, s.cannot_change, s.follow_only),
        Locale::En => format!("{quoted} {} {service} — {}", s.cannot_change_in, s.follow_only),
    }
}

/// `"Плейлист синхронизирован: +2 −1 здесь, 3 в Spotify"` /
/// `"Playlist synced: +2 −1 here, 3 in Spotify"`.
pub fn fmt_sync_done_in(loc: Locale, added: usize, removed: usize, there: usize, service: &str) -> String {
    let s = activity_in(loc);
    format!("{}: +{added} −{removed} {}, {there} {} {service}", s.sync_done, s.here, s.in_word)
}

/// `"4 трека"` / `"4 tracks"`: `n` with the plural form of `forms`.
pub fn fmt_count_in(loc: Locale, n: usize, forms: &PluralForms) -> String {
    format!("{n} {}", plural_forms(loc, n as u64, forms))
}

/// When a world was last heard: `"Слушал вчера"`, `"Слушал 4 дня назад"`,
/// `"Давно не слушал"`; `None` for never.
pub fn fmt_heard_when_in(loc: Locale, days: Option<i64>) -> String {
    let s = activity_in(loc);
    let Some(d) = days else { return s.heard_never.to_owned() };
    match d {
        ..=0 => s.heard_today.to_owned(),
        1 => s.heard_yesterday.to_owned(),
        2 => s.heard_day_before.to_owned(),
        3..=6 => format!(
            "{} {d} {} {}",
            s.heard_prefix,
            plural_forms(loc, d as u64, &s.days),
            s.heard_ago
        ),
        7..=9 => s.heard_week_ago.to_owned(),
        10..=17 => s.heard_two_weeks_ago.to_owned(),
        _ => s.heard_long_ago.to_owned(),
    }
}

/// A world card's title: `"Группа крови, трек 3"` / `"Gruppa Krovi, track 3"`.
pub fn fmt_track_number_in(loc: Locale, name: &str, n: usize) -> String {
    format!("{name}, {} {n}", activity_in(loc).track_one)
}

/// `"Новее всего"` / `"Новее всего, 2019"`.
pub fn fmt_newest_in(loc: Locale, year: Option<i32>) -> String {
    let s = activity_in(loc);
    match year {
        Some(y) => format!("{}, {y}", s.card_newest),
        None => s.card_newest.to_owned(),
    }
}

/// `"YouTube просит подождать — включи трек чуть позже"` /
/// `"YouTube asks to wait — try playing this track later"`.
pub fn fmt_rate_limited_in(loc: Locale, who: &str) -> String {
    match loc {
        Locale::Ru => format!("{who} просит подождать — включи трек чуть позже"),
        Locale::En => format!("{who} asks to wait — try playing this track later"),
    }
}

/// `"нет связи с YouTube — проверь интернет"` /
/// `"cannot reach YouTube — check your internet"`.
pub fn fmt_no_connection_in(loc: Locale, who: &str) -> String {
    match loc {
        Locale::Ru => format!("нет связи с {who} — проверь интернет"),
        Locale::En => format!("cannot reach {who} — check your internet"),
    }
}

/// `"Скопирована ссылка на YouTube Music"` / `"Copied the YouTube Music link"`.
pub fn fmt_link_copied_for_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Скопирована ссылка на {name}"),
        Locale::En => format!("Copied the {name} link"),
    }
}

/// `"Deezer не ответил — скопирована ссылка на YouTube Music"` /
/// `"Deezer did not answer — copied the YouTube Music link"`.
pub fn fmt_link_copied_after_in(loc: Locale, why: &str, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{why} — скопирована ссылка на {name}"),
        Locale::En => format!("{why} — copied the {name} link"),
    }
}

/// `"YouTube Music не ответил"` / `"YouTube Music did not answer"`.
pub fn fmt_service_no_answer_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} не ответил"),
        Locale::En => format!("{name} did not answer"),
    }
}

/// `"Spotify: 312 лайков теперь в Любимых"` /
/// `"Spotify: 312 likes now in Liked Songs"`.
pub fn fmt_first_likes_in(loc: Locale, service: &str, n: usize) -> String {
    let s = activity_in(loc);
    let likes = plural_forms(loc, n as u64, &s.likes_plural);
    match loc {
        Locale::Ru => format!("{service}: {n} {likes} теперь в Любимых"),
        Locale::En => format!("{service}: {n} {likes} now in Liked Songs"),
    }
}

/// `"Не получилось поставить лайк в Spotify"` /
/// `"Could not like in Spotify"`; `on` false for an unlike.
pub fn fmt_like_failed_in(loc: Locale, on: bool, service: &str) -> String {
    match (loc, on) {
        (Locale::Ru, true) => format!("Не получилось поставить лайк в {service}"),
        (Locale::Ru, false) => format!("Не получилось снять лайк в {service}"),
        (Locale::En, true) => format!("Could not like in {service}"),
        (Locale::En, false) => format!("Could not unlike in {service}"),
    }
}

/// `"Окно входа не открылось ({m}). Можно вставить токен: «Вставить токен…»"` /
/// `"The sign-in window did not open ({m}). You can paste a token: “Paste token…”"`.
pub fn fmt_sc_window_failed_in(loc: Locale, why: &str) -> String {
    match loc {
        Locale::Ru => format!("Окно входа не открылось ({why}). Можно вставить токен: «Вставить токен…»"),
        Locale::En => format!("The sign-in window did not open ({why}). You can paste a token: “Paste token…”"),
    }
}

/// `"Окно входа не открылось ({m}). Можно войти по cookies: «Вставить cookies…»"` /
/// `"The sign-in window did not open ({m}). You can sign in with cookies: “Paste cookies…”"`.
pub fn fmt_login_window_failed_in(loc: Locale, why: &str) -> String {
    match loc {
        Locale::Ru => format!("Окно входа не открылось ({why}). Можно войти по cookies: «Вставить cookies…»"),
        Locale::En => format!("The sign-in window did not open ({why}). You can sign in with cookies: “Paste cookies…”"),
    }
}

/// `"Заменено на «Группа крови»"` / `"Replaced with “Gruppa Krovi”"`.
pub fn fmt_replaced_with_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("Заменено на «{title}»"),
        Locale::En => format!("Replaced with “{title}”"),
    }
}

/// `"Spotify отвечает с задержкой — результаты появятся сами"` /
/// `"Spotify is slow to answer — results will appear on their own"`.
pub fn fmt_wait_short_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} отвечает с задержкой — результаты появятся сами"),
        Locale::En => format!("{name} is slow to answer — results will appear on their own"),
    }
}

/// `" до 14:30"` / `" until 14:30"`.
pub fn fmt_wait_until_in(loc: Locale, hm: &str) -> String {
    match loc {
        Locale::Ru => format!(" до {hm}"),
        Locale::En => format!(" until {hm}"),
    }
}

/// `"Spotify сейчас перегружен до 14:30"` /
/// `"Spotify is overloaded right now until 14:30"`.
pub fn fmt_wait_busy_in(loc: Locale, name: &str, until: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} сейчас перегружен{until}"),
        Locale::En => format!("{name} is overloaded right now{until}"),
    }
}

/// `"Spotify сейчас перегружен — ищу в Deezer"` /
/// `"Spotify is overloaded right now — searching Deezer"`.
pub fn fmt_wait_busy_other_in(loc: Locale, name: &str, until: &str, other: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} сейчас перегружен{until} — ищу в {other}"),
        Locale::En => format!("{name} is overloaded right now{until} — searching {other}"),
    }
}

/// `"Spotify сейчас перегружен — поищу ещё раз сам"` /
/// `"Spotify is overloaded right now — I'll try again myself"`.
pub fn fmt_wait_busy_retry_in(loc: Locale, name: &str, until: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} сейчас перегружен{until} — поищу ещё раз сам"),
        Locale::En => format!("{name} is overloaded right now{until} — I'll try again myself"),
    }
}

/// `"Spotify сейчас перегружен"` / `"Spotify is overloaded right now"`.
pub fn fmt_search_overloaded_in(loc: Locale, name: &str) -> String {
    format!("{name} {}", activity_in(loc).search_overloaded)
}

/// `"Нет связи с Spotify"` / `"Cannot reach Spotify"`.
pub fn fmt_search_no_connection_in(loc: Locale, name: &str) -> String {
    format!("{} {name}", activity_in(loc).search_no_connection)
}

/// `"Prowlarr отвечает · 12 трекеров"` / `"Prowlarr answers · 12 trackers"`.
pub fn fmt_prowlarr_answers_in(loc: Locale, n: usize) -> String {
    let s = activity_in(loc);
    let trackers = plural_forms(loc, n as u64, &s.trackers_plural);
    format!("{} · {n} {trackers}", s.prowlarr_answers)
}

/// `"Prowlarr ответил ошибкой 500"` / `"Prowlarr answered with error 500"`.
pub fn fmt_prowlarr_http_error_in(loc: Locale, status: u16) -> String {
    match loc {
        Locale::Ru => format!("Prowlarr ответил ошибкой {status}"),
        Locale::En => format!("Prowlarr answered with error {status}"),
    }
}

/// `"«Группа крови»: файл в Корзине, ищу заново"` /
/// `"“Gruppa Krovi”: the file is in the Trash, looking again"`.
pub fn fmt_file_in_trash_in(loc: Locale, title: &str) -> String {
    let quoted = fmt_quoted_in(loc, title);
    match loc {
        Locale::Ru => format!("{quoted}: файл в Корзине, ищу заново"),
        Locale::En => format!("{quoted}: the file is in the Trash, looking again"),
    }
}

/// `"Не получилось убрать файл «Группа крови»"` /
/// `"Could not remove the file “Gruppa Krovi”"`.
pub fn fmt_could_not_remove_file_in(loc: Locale, title: &str) -> String {
    let quoted = fmt_quoted_in(loc, title);
    match loc {
        Locale::Ru => format!("Не получилось убрать файл {quoted}"),
        Locale::En => format!("Could not remove the file {quoted}"),
    }
}

/// `"Объединено в «Группа крови»"` / `"Merged into “Gruppa Krovi”"`.
pub fn fmt_merged_into_in(loc: Locale, title: &str) -> String {
    let quoted = fmt_quoted_in(loc, title);
    match loc {
        Locale::Ru => format!("Объединено в {quoted}"),
        Locale::En => format!("Merged into {quoted}"),
    }
}

/// `"Устройство «Кухня» забыто"` / `"Device “Kitchen” forgotten"`.
pub fn fmt_device_forgotten_in(loc: Locale, name: &str) -> String {
    let quoted = fmt_quoted_in(loc, name);
    match loc {
        Locale::Ru => format!("Устройство {quoted} забыто"),
        Locale::En => format!("Device {quoted} forgotten"),
    }
}

/// `"«Моя волна» уже в библиотеке"` / `"“My Wave” is already in the library"`.
pub fn fmt_playlist_already_in_library_in(loc: Locale, name: &str) -> String {
    let quoted = fmt_quoted_in(loc, name);
    format!("{quoted} {}", activity_in(loc).playlist_already_in_library)
}

/// `"«Моя волна» в библиотеке"` / `"“My Wave” is in the library"`.
pub fn fmt_playlist_in_library_in(loc: Locale, name: &str) -> String {
    let quoted = fmt_quoted_in(loc, name);
    format!("{quoted} {}", activity_in(loc).playlist_in_library)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_counts_translate_and_pluralize() {
        assert_eq!(group_in(Locale::Ru, 1240), "1\u{a0}240");
        assert_eq!(group_in(Locale::En, 1240), "1,240");
        assert_eq!(group_in(Locale::Ru, 999), "999");
        assert_eq!(group_in(Locale::Ru, 1_000_000), "1\u{a0}000\u{a0}000");

        assert_eq!(fmt_import_counts_in(Locale::Ru, 0, 0, 0, 0), "ничего");
        assert_eq!(fmt_import_counts_in(Locale::En, 0, 0, 0, 0), "nothing");
        assert_eq!(
            fmt_import_counts_in(Locale::Ru, 1240, 312, 0, 9),
            "1\u{a0}240 прослушиваний, 312 лайков, 9 плейлистов"
        );
        assert_eq!(
            fmt_import_counts_in(Locale::En, 1240, 312, 0, 9),
            "1,240 plays, 312 likes, 9 playlists"
        );
        assert_eq!(fmt_import_counts_in(Locale::Ru, 3, 0, 0, 2), "3 прослушивания, 2 плейлиста");
        assert_eq!(fmt_import_counts_in(Locale::En, 3, 0, 0, 2), "3 plays, 2 playlists");
        assert_eq!(fmt_import_counts_in(Locale::Ru, 9, 0, 21, 0), "9 прослушиваний, 21 песня медиатеки");
        assert_eq!(fmt_import_counts_in(Locale::En, 9, 0, 21, 0), "9 plays, 21 library songs");
        assert_eq!(fmt_import_counts_in(Locale::Ru, 1, 1, 1, 1), "1 прослушивание, 1 лайк, 1 песня медиатеки, 1 плейлист");
        assert_eq!(fmt_import_counts_in(Locale::En, 1, 1, 1, 1), "1 play, 1 like, 1 library song, 1 playlist");

        assert_eq!(fmt_import_skipped_in(Locale::Ru, 0), "");
        assert_eq!(fmt_import_skipped_in(Locale::Ru, 1), " · пропущен 1 плейлист не с музыкой");
        assert_eq!(fmt_import_skipped_in(Locale::Ru, 2), " · пропущено 2 плейлиста не с музыкой");
        assert_eq!(fmt_import_skipped_in(Locale::Ru, 5), " · пропущено 5 плейлистов не с музыкой");
        assert_eq!(fmt_import_skipped_in(Locale::En, 1), " · 1 playlist skipped, not music");
        assert_eq!(fmt_import_skipped_in(Locale::En, 2), " · 2 playlists skipped, not music");

        assert_eq!(fmt_quoted_in(Locale::Ru, "X"), "«X»");
        assert_eq!(fmt_quoted_in(Locale::En, "X"), "“X”");
    }

    #[test]
    fn heard_when_and_world_cards_translate() {
        assert_eq!(fmt_heard_when_in(Locale::Ru, None), "Ещё не начинал");
        assert_eq!(fmt_heard_when_in(Locale::En, None), "Not started yet");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(0)), "Слушал сегодня");
        assert_eq!(fmt_heard_when_in(Locale::En, Some(0)), "Listened today");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(1)), "Слушал вчера");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(2)), "Слушал позавчера");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(3)), "Слушал 3 дня назад");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(4)), "Слушал 4 дня назад");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(5)), "Слушал 5 дней назад");
        assert_eq!(fmt_heard_when_in(Locale::En, Some(1)), "Listened 1 day ago");
        assert_eq!(fmt_heard_when_in(Locale::En, Some(4)), "Listened 4 days ago");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(8)), "Слушал неделю назад");
        assert_eq!(fmt_heard_when_in(Locale::En, Some(8)), "Listened a week ago");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(12)), "Слушал две недели назад");
        assert_eq!(fmt_heard_when_in(Locale::Ru, Some(40)), "Давно не слушал");
        assert_eq!(fmt_heard_when_in(Locale::En, Some(40)), "Not listened in a while");

        assert_eq!(fmt_count_in(Locale::Ru, 1, &activity_in(Locale::Ru).albums), "1 альбом");
        assert_eq!(fmt_count_in(Locale::Ru, 3, &activity_in(Locale::Ru).albums), "3 альбома");
        assert_eq!(fmt_count_in(Locale::Ru, 5, &activity_in(Locale::Ru).albums), "5 альбомов");
        assert_eq!(fmt_count_in(Locale::En, 1, &activity_in(Locale::En).albums), "1 album");
        assert_eq!(fmt_count_in(Locale::En, 3, &activity_in(Locale::En).albums), "3 albums");
        assert_eq!(fmt_count_in(Locale::Ru, 2, &activity_in(Locale::Ru).releases), "2 релиза");
        assert_eq!(fmt_count_in(Locale::En, 2, &activity_in(Locale::En).releases), "2 releases");

        assert_eq!(fmt_track_number_in(Locale::Ru, "Группа крови", 3), "Группа крови, трек 3");
        assert_eq!(fmt_track_number_in(Locale::En, "Gruppa Krovi", 3), "Gruppa Krovi, track 3");
        assert_eq!(fmt_newest_in(Locale::Ru, None), "Новее всего");
        assert_eq!(fmt_newest_in(Locale::Ru, Some(2019)), "Новее всего, 2019");
        assert_eq!(fmt_newest_in(Locale::En, Some(2019)), "Newest, 2019");
    }

    #[test]
    fn streams_shares_and_prowlarr_translate() {
        assert_eq!(fmt_rate_limited_in(Locale::Ru, "YouTube"), "YouTube просит подождать — включи трек чуть позже");
        assert_eq!(fmt_rate_limited_in(Locale::En, "YouTube"), "YouTube asks to wait — try playing this track later");
        assert_eq!(fmt_no_connection_in(Locale::Ru, "Spotify"), "нет связи с Spotify — проверь интернет");
        assert_eq!(fmt_no_connection_in(Locale::En, "Spotify"), "cannot reach Spotify — check your internet");

        assert_eq!(fmt_link_copied_for_in(Locale::Ru, "YouTube Music"), "Скопирована ссылка на YouTube Music");
        assert_eq!(fmt_link_copied_for_in(Locale::En, "YouTube Music"), "Copied the YouTube Music link");
        assert_eq!(
            fmt_link_copied_after_in(Locale::Ru, "Deezer не ответил", "YouTube Music"),
            "Deezer не ответил — скопирована ссылка на YouTube Music"
        );
        assert_eq!(
            fmt_link_copied_after_in(Locale::En, "Deezer did not answer", "YouTube Music"),
            "Deezer did not answer — copied the YouTube Music link"
        );
        assert_eq!(fmt_service_no_answer_in(Locale::Ru, "YouTube Music"), "YouTube Music не ответил");
        assert_eq!(fmt_service_no_answer_in(Locale::En, "YouTube Music"), "YouTube Music did not answer");

        assert_eq!(fmt_first_likes_in(Locale::Ru, "Spotify", 1), "Spotify: 1 лайк теперь в Любимых");
        assert_eq!(fmt_first_likes_in(Locale::Ru, "Spotify", 3), "Spotify: 3 лайка теперь в Любимых");
        assert_eq!(fmt_first_likes_in(Locale::Ru, "Spotify", 312), "Spotify: 312 лайков теперь в Любимых");
        assert_eq!(fmt_first_likes_in(Locale::En, "Spotify", 312), "Spotify: 312 likes now in Liked Songs");
        assert_eq!(fmt_like_failed_in(Locale::Ru, true, "Spotify"), "Не получилось поставить лайк в Spotify");
        assert_eq!(fmt_like_failed_in(Locale::Ru, false, "Spotify"), "Не получилось снять лайк в Spotify");
        assert_eq!(fmt_like_failed_in(Locale::En, true, "Spotify"), "Could not like in Spotify");
        assert_eq!(fmt_like_failed_in(Locale::En, false, "Spotify"), "Could not unlike in Spotify");

        assert_eq!(fmt_prowlarr_answers_in(Locale::Ru, 1), "Prowlarr отвечает · 1 трекер");
        assert_eq!(fmt_prowlarr_answers_in(Locale::Ru, 12), "Prowlarr отвечает · 12 трекеров");
        assert_eq!(fmt_prowlarr_answers_in(Locale::En, 12), "Prowlarr answers · 12 trackers");
        assert_eq!(fmt_prowlarr_http_error_in(Locale::Ru, 500), "Prowlarr ответил ошибкой 500");
        assert_eq!(fmt_prowlarr_http_error_in(Locale::En, 500), "Prowlarr answered with error 500");

        assert_eq!(fmt_wait_short_in(Locale::Ru, "Spotify"), "Spotify отвечает с задержкой — результаты появятся сами");
        assert_eq!(fmt_wait_until_in(Locale::Ru, "14:30"), " до 14:30");
        assert_eq!(fmt_wait_until_in(Locale::En, "14:30"), " until 14:30");
        assert_eq!(fmt_wait_busy_in(Locale::Ru, "Spotify", " до 14:30"), "Spotify сейчас перегружен до 14:30");
        assert_eq!(
            fmt_wait_busy_other_in(Locale::Ru, "Spotify", "", "Deezer"),
            "Spotify сейчас перегружен — ищу в Deezer"
        );
        assert_eq!(
            fmt_wait_busy_retry_in(Locale::En, "Spotify", ""),
            "Spotify is overloaded right now — I'll try again myself"
        );
        assert_eq!(fmt_search_overloaded_in(Locale::Ru, "Spotify"), "Spotify сейчас перегружен");
        assert_eq!(fmt_search_no_connection_in(Locale::En, "Spotify"), "Cannot reach Spotify");

        assert_eq!(fmt_replaced_with_in(Locale::Ru, "Группа крови"), "Заменено на «Группа крови»");
        assert_eq!(fmt_replaced_with_in(Locale::En, "Gruppa Krovi"), "Replaced with “Gruppa Krovi”");
        assert_eq!(fmt_merged_into_in(Locale::Ru, "Группа крови"), "Объединено в «Группа крови»");
        assert_eq!(fmt_device_forgotten_in(Locale::Ru, "Кухня"), "Устройство «Кухня» забыто");
        assert_eq!(fmt_device_forgotten_in(Locale::En, "Kitchen"), "Device “Kitchen” forgotten");
        assert_eq!(
            fmt_playlist_already_in_library_in(Locale::Ru, "Моя волна"),
            "«Моя волна» уже в библиотеке"
        );
        assert_eq!(fmt_playlist_in_library_in(Locale::En, "My Wave"), "“My Wave” is in the library");
        assert_eq!(fmt_file_in_trash_in(Locale::Ru, "Группа крови"), "«Группа крови»: файл в Корзине, ищу заново");
        assert_eq!(
            fmt_could_not_remove_file_in(Locale::Ru, "Группа крови"),
            "Не получилось убрать файл «Группа крови»"
        );
        assert_eq!(
            fmt_sc_window_failed_in(Locale::Ru, "no WebView2"),
            "Окно входа не открылось (no WebView2). Можно вставить токен: «Вставить токен…»"
        );
        assert_eq!(
            fmt_login_window_failed_in(Locale::En, "no WebView2"),
            "The sign-in window did not open (no WebView2). You can sign in with cookies: “Paste cookies…”"
        );
    }
}
