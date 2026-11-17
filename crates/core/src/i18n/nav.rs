//! Navigation, library tabs, virtual playlists and system tray strings.


/// Static navigation and tray strings.
pub struct NavStrings {
    pub home: &'static str,
    pub library: &'static str,
    pub downloads: &'static str,
    pub settings: &'static str,
    pub liked: &'static str,
    pub liked_albums: &'static str,
    pub tray_show_hide: &'static str,
    pub tray_play: &'static str,
    pub tray_pause: &'static str,
    pub tray_next: &'static str,
    pub tray_previous: &'static str,
    pub tray_quit: &'static str,
}

pub static RU: NavStrings = NavStrings {
    home: "Главная",
    library: "Библиотека",
    downloads: "Загрузки",
    settings: "Настройки",
    liked: "Любимые",
    liked_albums: "Любимые альбомы",
    tray_show_hide: "Показать / скрыть",
    tray_play: "Играть",
    tray_pause: "Пауза",
    tray_next: "Следующий",
    tray_previous: "Предыдущий",
    tray_quit: "Выход",
};

pub static EN: NavStrings = NavStrings {
    home: "Home",
    library: "Library",
    downloads: "Downloads",
    settings: "Settings",
    liked: "Liked Songs",
    liked_albums: "Liked Albums",
    tray_show_hide: "Show / Hide",
    tray_play: "Play",
    tray_pause: "Pause",
    tray_next: "Next",
    tray_previous: "Previous",
    tray_quit: "Quit",
};

locale_table! {
    /// Returns navigation strings for `loc`.
    nav_in -> NavStrings,
    /// Returns navigation strings for the active process locale.
    nav,
    { Ru => RU, En => EN }
}
