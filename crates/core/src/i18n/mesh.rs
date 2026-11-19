//! Mesh (devices, network, pairing, replicas) UI and errors.

use super::{Locale, PluralForms, plural_forms};

/// Static strings: mesh (devices, network, pairing, replicas) UI and errors.
pub struct MeshStrings {
    /// «1,2 из 27,6 ГБ»: the word between the two sizes.
    pub size_of: &'static str,
    /// A copied file whose hash did not match.
    pub file_damaged: &'static str,
    /// A device's kind in words: «Компьютер», «Raspberry Pi», «Телефон»…
    pub kind_pc: &'static str,
    pub kind_pi: &'static str,
    pub kind_phone: &'static str,
    pub kind_tv: &'static str,
    pub kind_server: &'static str,
    /// A device's storage profile: «Полная копия»…
    pub profile_full: &'static str,
    pub profile_library: &'static str,
    /// The chip of «Избранное…» in a profile menu.
    pub profile_favorites_ellipsis: &'static str,
    pub profile_sound_only: &'static str,
    pub this_computer: &'static str,
    pub guest: &'static str,
    pub map_title: &'static str,
    pub play_here: &'static str,
    pub remote: &'static str,
    pub stores: &'static str,
    pub playing: &'static str,
    pub version: &'static str,
    pub address: &'static str,
    pub last_seen: &'static str,
    pub connect: &'static str,
    pub install: &'static str,
    pub add_device: &'static str,
    pub forget: &'static str,
    pub forget_anyway: &'static str,
    pub forget_device: &'static str,
    pub name_label: &'static str,
    pub favorites: &'static str,
    pub liked: &'static str,
    /// The originals a device's line notes.
    pub masters: PluralForms,
    pub close_esc: &'static str,
    /// What keeps a device from being added, and why a pairing failed.
    pub links_off: &'static str,
    pub mesh_not_running: &'static str,
    pub mesh_no_answer: &'static str,
    pub mesh_off: &'static str,
    pub api_off: &'static str,
    pub turn_on: &'static str,
    pub local_only: &'static str,
    pub enable_home_network: &'static str,
    pub only_holder_adds: &'static str,
    pub only_holder_pairs: &'static str,
    pub platform_linux: &'static str,
    pub no_lan_addr: &'static str,
    pub wrong_code: &'static str,
    pub invite_gone: &'static str,
    pub invite_cancelled: &'static str,
    pub device_busy: &'static str,
    pub device_paired: &'static str,
    pub ring_full: &'static str,
    pub bad_answer: &'static str,
    pub no_link: &'static str,
    pub pair_save_failed: &'static str,
    pub not_a_ticket: &'static str,
    pub newer_invite: &'static str,
    pub invite_expired: &'static str,
    pub invite_not_made: &'static str,
    pub ring_not_made: &'static str,
    pub no_such_device: &'static str,
    pub code_six_digits: &'static str,
    pub device_gone: &'static str,
    pub already_paired: &'static str,
    /// A forget's failure, said inside another sentence (lowercase).
    pub forget_no_device: &'static str,
    pub forget_only_holder: &'static str,
    pub forget_not_linked: &'static str,
    pub forget_ring_disk: &'static str,
    pub player_silent: &'static str,
    /// The library's sync with a device, on its row.
    pub sync_blocked: &'static str,
    pub sync_refused: &'static str,
    pub sync_clock_ahead: &'static str,
    pub sync_failed: &'static str,
    /// A request to a device, in a toast.
    pub ask_not_in_ring: &'static str,
    pub ask_unreachable: &'static str,
    pub ask_request_failed: &'static str,
    pub ask_no_answer: &'static str,
    pub ask_api_down: &'static str,
    pub ask_player_silent: &'static str,
    pub ask_busy_now: &'static str,
}

pub static RU: MeshStrings = MeshStrings {
    size_of: "из",
    file_damaged: "файл пришёл повреждённым",
    kind_pc: "Компьютер",
    kind_pi: "Raspberry Pi",
    kind_phone: "Телефон",
    kind_tv: "Телевизор",
    kind_server: "Сервер",
    profile_full: "Полная копия",
    profile_library: "Библиотека",
    profile_favorites_ellipsis: "Избранное…",
    profile_sound_only: "Только звук",
    this_computer: "Этот компьютер",
    guest: "Гость",
    map_title: "Карта сети",
    play_here: "Слушать здесь",
    remote: "Пульт",
    stores: "Хранит",
    playing: "Играет",
    version: "Версия",
    address: "Адрес",
    last_seen: "Был в сети",
    connect: "Подключить",
    install: "Установить",
    add_device: "Добавить устройство",
    forget: "Забыть",
    forget_anyway: "Забыть всё равно",
    forget_device: "Забыть устройство",
    name_label: "Название",
    favorites: "Избранное",
    liked: "Любимые",
    masters: PluralForms { one: "оригинал", few: "оригинала", many: "оригиналов" },
    close_esc: "Закрыть (Esc)",
    links_off: "Связь с устройствами выключена",
    mesh_not_running: "Межсеть не запущена",
    mesh_no_answer: "Межсеть не ответила",
    mesh_off: "Межсеть выключена",
    api_off: "API выключен",
    turn_on: "Включить",
    local_only: "Доступ открыт только на этом компьютере",
    enable_home_network: "Включить домашнюю сеть",
    only_holder_adds: "Добавлять устройства может только главное устройство",
    only_holder_pairs: "Привязывать может только главное устройство",
    platform_linux: "Linux и Raspberry Pi",
    no_lan_addr: "У компьютера нет адреса в домашней сети",
    wrong_code: "Неверный код",
    invite_gone: "Приглашение истекло или уже использовано",
    invite_cancelled: "Приглашение отменено: слишком много неверных попыток",
    device_busy: "Устройство занято другой привязкой",
    device_paired: "Устройство уже привязано",
    ring_full: "В кольце нет места для нового устройства",
    bad_answer: "Устройство ответило не так",
    no_link: "Нет связи с устройством",
    pair_save_failed: "Не удалось сохранить привязку",
    not_a_ticket: "Это не приглашение",
    newer_invite: "Приглашение от более новой версии",
    invite_expired: "Приглашение истекло",
    invite_not_made: "Не удалось создать приглашение",
    ring_not_made: "Не удалось создать кольцо",
    no_such_device: "Нет такого устройства",
    code_six_digits: "Код — шесть цифр",
    device_gone: "Устройство пропало из сети",
    already_paired: "Это устройство уже привязано",
    forget_no_device: "нет такого устройства",
    forget_only_holder: "забывать устройства может только главное устройство",
    forget_not_linked: "это устройство ни с кем не связано",
    forget_ring_disk: "не удалось записать кольцо на диск",
    player_silent: "Плеер не ответил: он запущен, и межсеть в нём включена?",
    sync_blocked: "Синхронизация остановлена: метка базы не записана",
    sync_refused: "Устройство не принимает синхронизацию",
    sync_clock_ahead: "Часы устройства спешат",
    sync_failed: "Синхронизация не удалась",
    ask_not_in_ring: "устройство не в кольце",
    ask_unreachable: "устройство недоступно",
    ask_request_failed: "запрос не прошёл",
    ask_no_answer: "устройство не ответило",
    ask_api_down: "API этого устройства не запущен",
    ask_player_silent: "плеер не ответил",
    ask_busy_now: "слишком много дел сразу",
};

pub static EN: MeshStrings = MeshStrings {
    size_of: "of",
    file_damaged: "the file arrived damaged",
    kind_pc: "Computer",
    kind_pi: "Raspberry Pi",
    kind_phone: "Phone",
    kind_tv: "TV",
    kind_server: "Server",
    profile_full: "Full copy",
    profile_library: "Library",
    profile_favorites_ellipsis: "Favorites…",
    profile_sound_only: "Sound only",
    this_computer: "This computer",
    guest: "Guest",
    map_title: "Network map",
    play_here: "Play here",
    remote: "Remote",
    stores: "Stores",
    playing: "Playing",
    version: "Version",
    address: "Address",
    last_seen: "Last seen",
    connect: "Connect",
    install: "Install",
    add_device: "Add device",
    forget: "Forget",
    forget_anyway: "Forget anyway",
    forget_device: "Forget device",
    name_label: "Name",
    favorites: "Favorites",
    liked: "Liked Songs",
    masters: PluralForms { one: "master", few: "masters", many: "masters" },
    close_esc: "Close (Esc)",
    links_off: "Device links are off",
    mesh_not_running: "Mesh is not running",
    mesh_no_answer: "Mesh did not respond",
    mesh_off: "Mesh is off",
    api_off: "The API is off",
    turn_on: "Enable",
    local_only: "Access is limited to this computer",
    enable_home_network: "Enable home network",
    only_holder_adds: "Only the main device can add devices",
    only_holder_pairs: "Only the main device can pair",
    platform_linux: "Linux and Raspberry Pi",
    no_lan_addr: "This computer has no address on the home network",
    wrong_code: "Wrong code",
    invite_gone: "The invitation has expired or was already used",
    invite_cancelled: "Invitation cancelled: too many incorrect attempts",
    device_busy: "The device is busy with another pairing",
    device_paired: "The device is already paired",
    ring_full: "No room for another device in the ring",
    bad_answer: "Unexpected response from the device",
    no_link: "Cannot connect to the device",
    pair_save_failed: "Could not save the pairing",
    not_a_ticket: "Not an invitation",
    newer_invite: "Invitation from a newer version",
    invite_expired: "The invitation has expired",
    invite_not_made: "Could not create the invitation",
    ring_not_made: "Could not create the ring",
    no_such_device: "No such device",
    code_six_digits: "The code is six digits",
    device_gone: "The device left the network",
    already_paired: "This device is already paired",
    forget_no_device: "no such device",
    forget_only_holder: "only the main device can forget devices",
    forget_not_linked: "this device is not linked to any others",
    forget_ring_disk: "could not save the ring to disk",
    player_silent: "The player did not respond. Is it running with Mesh enabled?",
    sync_blocked: "Sync stopped: the database sync marker was not saved",
    sync_refused: "The device is not accepting sync",
    sync_clock_ahead: "The device's clock is ahead",
    sync_failed: "Sync failed",
    ask_not_in_ring: "the device is not in the ring",
    ask_unreachable: "the device cannot be reached",
    ask_request_failed: "the request failed",
    ask_no_answer: "the device did not respond",
    ask_api_down: "this device's API is not running",
    ask_player_silent: "the player did not respond",
    ask_busy_now: "the device is busy",
};

locale_table! {
    /// Returns these strings for `loc`.
    mesh_in -> MeshStrings,
    /// Returns these strings for the active process locale.
    mesh,
    { Ru => RU, En => EN }
}


/// The choices of a device's profile menu.
pub const fn profile_choices_in(loc: Locale) -> [&'static str; 4] {
    match loc {
        Locale::Ru => ["Полная копия", "Библиотека", "Избранное…", "Только звук"],
        Locale::En => ["Full copy", "Library", "Favorites…", "Sound only"],
    }
}

/// A device's second line when its disk is nearly full: «Мало места: свободно 1 ГБ».
pub fn fmt_low_space_in(loc: Locale, free: &str) -> String {
    match loc {
        Locale::Ru => format!("Мало места: свободно {free}"),
        Locale::En => format!("Low on space: {free} free"),
    }
}

/// This computer's second line: «296 ГБ · оригиналы 112 ГБ».
pub fn fmt_masters_in(loc: Locale, music: &str, masters: &str) -> String {
    match loc {
        Locale::Ru => format!("{music} · оригиналы {masters}"),
        Locale::En => format!("{music} · masters {masters}"),
    }
}

/// The passport's «свободно 12 ГБ».
pub fn fmt_free_in(loc: Locale, free: &str) -> String {
    match loc {
        Locale::Ru => format!("свободно {free}"),
        Locale::En => format!("{free} free"),
    }
}

/// «Забыть «Pi»?» of the forget's dialog.
pub fn fmt_forget_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Забыть «{name}»?"),
        Locale::En => format!("Forget “{name}”?"),
    }
}

/// What the forget's dialog says about the originals only that device holds.
pub fn fmt_forget_text_in(loc: Locale, originals: Option<u64>) -> String {
    match (loc, originals) {
        (Locale::Ru, Some(n)) => {
            let what = plural_forms(loc, n, &mesh_in(loc).masters);
            format!("Только на этом устройстве лежат проверенные копии: {n} {what} FLAC.")
        }
        (Locale::En, Some(n)) => {
            let what = plural_forms(loc, n, &mesh_in(loc).masters);
            format!("This device holds the only verified copies: {n} FLAC {what}.")
        }
        (Locale::Ru, None) => "Не удалось проверить, есть ли на этом устройстве единственные копии оригиналов FLAC.".to_owned(),
        (Locale::En, None) => "Could not check whether this device holds the only copies of the master FLAC files.".to_owned(),
    }
}

/// «Не удалось открыть сеть: {e}» when the mesh's endpoint cannot be bound.
pub fn fmt_open_net_in(loc: Locale, e: &str) -> String {
    match loc {
        Locale::Ru => format!("Не удалось открыть сеть: {e}"),
        Locale::En => format!("Could not open the network: {e}"),
    }
}

/// The sync's blocker of rows under another id: «Не все изменения приняты: N».
pub fn fmt_sync_conflicts_in(loc: Locale, n: u32) -> String {
    match loc {
        Locale::Ru => format!("Не все изменения приняты: {n}"),
        Locale::En => format!("Not all changes accepted: {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_choices_and_sync_words_say_themselves() {
        assert_eq!(profile_choices_in(Locale::Ru), ["Полная копия", "Библиотека", "Избранное…", "Только звук"]);
        assert_eq!(profile_choices_in(Locale::En), ["Full copy", "Library", "Favorites…", "Sound only"]);
        assert_eq!(fmt_sync_conflicts_in(Locale::Ru, 3), "Не все изменения приняты: 3");
        assert_eq!(fmt_sync_conflicts_in(Locale::En, 3), "Not all changes accepted: 3");
        assert_eq!(fmt_open_net_in(Locale::Ru, "x"), "Не удалось открыть сеть: x");
        assert_eq!(fmt_open_net_in(Locale::En, "x"), "Could not open the network: x");
    }

    #[test]
    fn storage_and_questions_say_themselves_in_both_languages() {
        assert_eq!(fmt_low_space_in(Locale::Ru, "1 ГБ"), "Мало места: свободно 1 ГБ");
        assert_eq!(fmt_low_space_in(Locale::En, "1 GB"), "Low on space: 1 GB free");
        assert_eq!(fmt_masters_in(Locale::Ru, "296 ГБ", "112 ГБ"), "296 ГБ · оригиналы 112 ГБ");
        assert_eq!(fmt_masters_in(Locale::En, "296 GB", "112 GB"), "296 GB · masters 112 GB");
        assert_eq!(fmt_free_in(Locale::Ru, "12 ГБ"), "свободно 12 ГБ");
        assert_eq!(fmt_free_in(Locale::En, "12 GB"), "12 GB free");
        assert_eq!(fmt_forget_in(Locale::Ru, "Pi"), "Забыть «Pi»?");
        assert_eq!(fmt_forget_in(Locale::En, "Pi"), "Forget “Pi”?");
    }

    #[test]
    fn the_forget_says_what_would_go_in_both_languages() {
        assert_eq!(
            fmt_forget_text_in(Locale::Ru, Some(12)),
            "Только на этом устройстве лежат проверенные копии: 12 оригиналов FLAC."
        );
        assert_eq!(
            fmt_forget_text_in(Locale::Ru, Some(1)),
            "Только на этом устройстве лежат проверенные копии: 1 оригинал FLAC."
        );
        assert_eq!(
            fmt_forget_text_in(Locale::En, Some(2)),
            "This device holds the only verified copies: 2 FLAC masters."
        );
        assert!(fmt_forget_text_in(Locale::Ru, None).starts_with("Не удалось проверить"));
        assert!(fmt_forget_text_in(Locale::En, None).starts_with("Could not check"));
    }
}
