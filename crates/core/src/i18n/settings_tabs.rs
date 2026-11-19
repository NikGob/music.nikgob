//! Settings tab pages (music, lyrics, look, services, downloads, network, system, devices, dev).

/// Static strings: settings tab pages (music, lyrics, look, services, downloads, network, system, devices, dev).
pub struct SettingsTabsStrings {}

pub static RU: SettingsTabsStrings = SettingsTabsStrings {};

pub static EN: SettingsTabsStrings = SettingsTabsStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    settings_tabs_in -> SettingsTabsStrings,
    /// Returns these strings for the active process locale.
    settings_tabs,
    { Ru => RU, En => EN }
}
