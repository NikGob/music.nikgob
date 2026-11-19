//! Phone UI (crates/mobile).

use std::time::Duration;

use super::{Locale, common_in, locale};

/// Static strings: phone UI (crates/mobile).
pub struct MobileStrings {
    // The output sheet (crates/mobile output.rs, host.rs).
    pub this_phone: &'static str,
    pub phone_device_name: &'static str,
    pub network_map: &'static str,
    // Shared parts (crates/mobile parts.rs).
    pub nothing_found: &'static str,
    // Dialogs (queue.rs, menu.rs, library.rs).
    pub save_queue_title: &'static str,
    pub new_playlist: &'static str,
    pub name_placeholder: &'static str,
    pub enter_name: &'static str,
    pub create: &'static str,
    // Library chips and tools (library.rs).
    pub chip_playlists: &'static str,
    pub chip_albums: &'static str,
    pub chip_artists: &'static str,
    pub chip_tracks: &'static str,
    pub layout_grid: &'static str,
    pub layout_list: &'static str,
    pub sort_by_artist: &'static str,
    pub sort_by_title: &'static str,
    pub sort_by_album: &'static str,
    pub sort_by_year: &'static str,
    pub sort_recently_added: &'static str,
    pub sort_by_duration: &'static str,
    // Search sections (search.rs).
    pub section_top_result: &'static str,
    // Settings (settings.rs).
    pub all_settings: &'static str,
    pub group_look: &'static str,
    pub theme_system: &'static str,
    pub theme_dark: &'static str,
    pub theme_light: &'static str,
    pub reduced_motion: &'static str,
    pub sign_out: &'static str,
    pub group_mesh: &'static str,
    pub my_devices: &'static str,
    pub group_favorites: &'static str,
    pub invitation: &'static str,
    pub connect: &'static str,
    // The track menu's tiles (menu.rs).
    pub tile_play_next: &'static str,
    pub tile_to_queue: &'static str,
    pub tile_to_playlist: &'static str,
    // Detail pages (detail.rs).
    pub artist_radio: &'static str,
    pub popular_tracks: &'static str,
    pub singles_eps: &'static str,
}

pub static RU: MobileStrings = MobileStrings {
    this_phone: "Этот телефон",
    phone_device_name: "Телефон",
    network_map: "Карта сети",
    nothing_found: "Ничего не нашлось",
    save_queue_title: "Сохранить как плейлист",
    new_playlist: "Новый плейлист",
    name_placeholder: "Название",
    enter_name: "Введи название",
    create: "Создать",
    chip_playlists: "Плейлисты",
    chip_albums: "Альбомы",
    chip_artists: "Исполнители",
    chip_tracks: "Треки",
    layout_grid: "Сетка",
    layout_list: "Список",
    sort_by_artist: "По исполнителю",
    sort_by_title: "По названию",
    sort_by_album: "По альбому",
    sort_by_year: "По году",
    sort_recently_added: "Недавно добавленные",
    sort_by_duration: "По длительности",
    section_top_result: "Лучший результат",
    all_settings: "Все настройки",
    group_look: "Вид",
    theme_system: "Системная",
    theme_dark: "Тёмная",
    theme_light: "Светлая",
    reduced_motion: "Меньше движения",
    sign_out: "Выйти",
    group_mesh: "Межсеть",
    my_devices: "Мои устройства",
    group_favorites: "Избранное",
    invitation: "Приглашение",
    connect: "Подключить",
    tile_play_next: "Следующим",
    tile_to_queue: "В очередь",
    tile_to_playlist: "В плейлист",
    artist_radio: "Радио исполнителя",
    popular_tracks: "Популярные треки",
    singles_eps: "Синглы и EP",
};

pub static EN: MobileStrings = MobileStrings {
    this_phone: "This phone",
    phone_device_name: "Phone",
    network_map: "Network map",
    nothing_found: "Nothing found",
    save_queue_title: "Save as playlist",
    new_playlist: "New playlist…",
    name_placeholder: "Name",
    enter_name: "Enter a name",
    create: "Create",
    chip_playlists: "Playlists",
    chip_albums: "Albums",
    chip_artists: "Artists",
    chip_tracks: "Tracks",
    layout_grid: "Grid",
    layout_list: "List",
    sort_by_artist: "By artist",
    sort_by_title: "By title",
    sort_by_album: "By album",
    sort_by_year: "By year",
    sort_recently_added: "Recently added",
    sort_by_duration: "By duration",
    section_top_result: "Top result",
    all_settings: "All settings",
    group_look: "Appearance",
    theme_system: "System",
    theme_dark: "Dark",
    theme_light: "Light",
    reduced_motion: "Reduced motion",
    sign_out: "Sign out",
    group_mesh: "Mesh",
    my_devices: "My devices",
    group_favorites: "Favorites",
    invitation: "Invitation",
    connect: "Connect",
    tile_play_next: "Play next",
    tile_to_queue: "Add to queue",
    tile_to_playlist: "Add to playlist…",
    artist_radio: "Artist radio",
    popular_tracks: "Popular tracks",
    singles_eps: "Singles and EPs",
};

locale_table! {
    /// Returns these strings for `loc`.
    mobile_in -> MobileStrings,
    /// Returns these strings for the active process locale.
    mobile,
    { Ru => RU, En => EN }
}

/// A total length as the phone writes it: «52 мин», «1 ч 12 мин» ("52 min", "1 hr 12 min").
pub fn fmt_total_in(loc: Locale, d: Duration) -> String {
    let c = common_in(loc);
    let m = (d.as_secs() + 30) / 60;
    if m >= 60 {
        format!("{} {} {} {}", m / 60, c.hr_short, m % 60, c.min_short)
    } else {
        format!("{m} {}", c.min_short)
    }
}

/// Formats a total length in the active locale.
pub fn fmt_total(d: Duration) -> String {
    fmt_total_in(locale(), d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_exist_in_both_locales() {
        for loc in Locale::ALL {
            let m = mobile_in(loc);
            assert!(!m.this_phone.is_empty() && !m.chip_tracks.is_empty() && !m.connect.is_empty());
            assert!(!m.save_queue_title.is_empty() && !m.artist_radio.is_empty() && !m.singles_eps.is_empty());
        }
        assert_eq!(mobile_in(Locale::Ru).this_phone, "Этот телефон");
        assert_eq!(mobile_in(Locale::En).this_phone, "This phone");
        assert_eq!(mobile_in(Locale::Ru).chip_tracks, "Треки");
        assert_eq!(mobile_in(Locale::En).chip_tracks, "Tracks");
        assert_eq!(mobile_in(Locale::Ru).tile_to_queue, "В очередь");
        assert_eq!(mobile_in(Locale::En).tile_to_queue, "Add to queue");
    }

    #[test]
    fn totals_read_both_locales() {
        assert_eq!(fmt_total_in(Locale::Ru, Duration::from_secs(52 * 60 + 10)), "52 мин");
        assert_eq!(fmt_total_in(Locale::Ru, Duration::from_secs(72 * 60)), "1 ч 12 мин");
        assert_eq!(fmt_total_in(Locale::En, Duration::from_secs(52 * 60 + 10)), "52 min");
        assert_eq!(fmt_total_in(Locale::En, Duration::from_secs(72 * 60)), "1 hr 12 min");
    }
}
