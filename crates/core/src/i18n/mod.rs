//! Compile-time checked localization (`i18n`) without runtime hash maps or allocations for static strings.
//!
//! Static labels live in per-locale structs (`&'static str`), so adding a new
//! [`Locale`] variant causes `rustc` to flag every untranslated entry. The
//! active UI locale sits in a lock-free [`AtomicU8`] slot (`0 ns` reads per
//! frame), while every accessor also has an explicit `*_in(Locale)` variant for
//! deterministic unit tests.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// Defines `<prefix>_in(Locale)` using an exhaustive `match loc` and `<prefix>()`
/// reading the active process locale. Adding a new [`Locale`] variant fails to
/// compile in every domain module until its table is added.
macro_rules! locale_table {
    (
        $(#[$in_meta:meta])*
        $in_fn:ident -> $ty:ty,
        $(#[$cur_meta:meta])*
        $cur_fn:ident,
        { $($variant:ident => $table:expr),+ $(,)? }
    ) => {
        $(#[$in_meta])*
        #[inline]
        pub const fn $in_fn(loc: $crate::i18n::Locale) -> &'static $ty {
            match loc {
                $($crate::i18n::Locale::$variant => &$table,)+
            }
        }

        $(#[$cur_meta])*
        #[inline]
        pub fn $cur_fn() -> &'static $ty {
            $in_fn($crate::i18n::locale())
        }
    };
}

pub mod activity;
pub mod backend;
pub mod common;
pub mod detail;
pub mod downloads;
pub mod errors;
pub mod first_run;
pub mod home_recs;
pub mod library;
pub mod lyrics;
pub mod menus;
pub mod mesh;
pub mod mobile;
pub mod nav;
pub mod pages;
pub mod player;
pub mod screens;
pub mod search;
pub mod settings;
pub mod settings_tabs;
pub mod status;
pub mod streams_ui;
pub mod view;

pub use common::{CommonStrings, common, common_in, fmt_total, fmt_total_in, hours_minutes, hours_minutes_in, tracks_count, tracks_count_in};
pub use settings_tabs::{SettingsTabsStrings, settings_tabs, settings_tabs_in};
pub use first_run::{FirstRunStrings, first_run, first_run_in};
pub use menus::{MenusStrings, menus, menus_in};
pub use library::{LibraryStrings, library, library_in};
pub use detail::{DetailStrings, detail, detail_in};
pub use search::{SearchStrings, search, search_in};
pub use pages::{PagesStrings, pages, pages_in};
pub use downloads::{DownloadsStrings, downloads, downloads_in};
pub use status::{StatusStrings, status, status_in};
pub use streams_ui::{StreamsUiStrings, streams_ui, streams_ui_in};
pub use mesh::{MeshStrings, mesh, mesh_in};
pub use screens::{ScreensStrings, screens, screens_in};
pub use view::{ViewStrings, view, view_in};
pub use mobile::{MobileStrings, mobile, mobile_in};
pub use activity::{ActivityStrings, activity, activity_in};
pub use backend::{BackendStrings, backend, backend_in};
pub use errors::{ErrorStrings, errors, errors_in};
pub use home_recs::{
    BackgroundVariant, HomeRecsStrings, MixTitle, MoodKind, SideMixFlavor, TileStatic, WorldMixKind,
    day_part_title_in, genre_family_title_in, home_recs, home_recs_in,
};
pub use lyrics::{LyricsStrings, lyrics, lyrics_in};
pub use nav::{NavStrings, nav, nav_in};
pub use player::{
    PlayerStrings, QueueOrigin, RadioOrigin, RadioTitle, SoundFactStrings, fmt_khz_in, fmt_mb_in, player,
    player_in, sound_facts, sound_facts_in,
};
pub use settings::{GroupId, SettingsStrings, lang_pref_label, lang_pref_label_in, settings, settings_in};

/// User language preference stored in `prefs.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LangPref {
    #[default]
    Auto,
    Ru,
    En,
}

impl LangPref {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Ru, Self::En];

    /// Resolves `self` to a concrete [`Locale`], falling back to `system` for [`Self::Auto`].
    pub const fn resolve(self, system: Locale) -> Locale {
        match self {
            Self::Auto => system,
            Self::Ru => Locale::Ru,
            Self::En => Locale::En,
        }
    }

    /// Resolves `self` against [`system_locale`] (avoiding OS queries for explicit languages).
    pub fn resolve_system(self) -> Locale {
        match self {
            Self::Auto => system_locale(),
            Self::Ru => Locale::Ru,
            Self::En => Locale::En,
        }
    }

    /// Display label for `self` in UI locale `loc`: [`Self::Auto`] is
    /// localized, while concrete languages always return their native endonym
    /// so users can recognize their language regardless of the active UI locale.
    pub const fn label_in(self, loc: Locale) -> &'static str {
        match self {
            Self::Auto => settings_in(loc).lang_auto,
            Self::Ru => "Русский",
            Self::En => "English (beta)",
        }
    }
}

macro_rules! define_locales {
    ($($variant:ident = $id:expr => ($code:expr, $plural:expr)),+ $(,)?) => {
        /// Concrete resolved process locale (never `Auto`).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum Locale {
            $($variant = $id,)+
        }

        impl Locale {
            const COUNT: usize = [$(Self::$variant,)+].len();

            /// Every supported process locale in canonical order.
            pub const ALL: [Self; Self::COUNT] = [$(Self::$variant,)+];

            /// Numeric discriminant stored in the lock-free atomic locale slot.
            pub const fn to_u8(self) -> u8 {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            /// Converts a raw `u8` into [`Locale`], returning `None` for invalid/uninitialized values.
            pub const fn try_from_u8(v: u8) -> Option<Self> {
                match v {
                    $($id => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// Converts a raw `u8` from the atomic slot back into [`Locale`].
            pub const fn from_u8(v: u8) -> Self {
                match Self::try_from_u8(v) {
                    Some(loc) => loc,
                    None => Self::Ru,
                }
            }

            /// BCP-47 short language code (e.g. `"ru"`, `"en"`).
            pub const fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }

            /// Pluralization rule for `self`.
            pub const fn plural_rule(self) -> PluralRule {
                match self {
                    $(Self::$variant => $plural,)+
                }
            }
        }
    };
}

define_locales! {
    Ru = 0 => ("ru", PluralRule::EastSlavic),
    En = 1 => ("en", PluralRule::OneOther),
}

/// Grammar pluralization rule used by [`plural`] and [`plural_forms`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluralRule {
    /// East Slavic (Russian, Ukrainian, Belarusian): `one` (1, 21…), `few` (2..=4, 22..=24…), `many` (0, 5..=20, 25…).
    EastSlavic,
    /// West Slavic (Polish, Czech): `one` (`n == 1`), `few` (2..=4, 22..=24… excl. 12..=14), `many` (0, 5..=21, 25…).
    WestSlavic,
    /// English-style (En, De, Es, It, Nl, Pt): `one` (`n == 1`), `many` (everything else).
    OneOther,
    /// French-style (Fr): `one` (`0 | 1`), `many` (everything else).
    OneZeroOther,
    /// No plural inflection (Ja, Ko, Zh, Tr).
    Invariant,
}

impl PluralRule {
    /// Selects the plural form of `n` under `self`.
    pub const fn select<'a>(self, n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
        match self {
            Self::EastSlavic => {
                let (n10, n100) = (n % 10, n % 100);
                if n10 == 1 && n100 != 11 {
                    one
                } else if n10 >= 2 && n10 <= 4 && !(n100 >= 12 && n100 <= 14) {
                    few
                } else {
                    many
                }
            }
            Self::WestSlavic => {
                if n == 1 {
                    one
                } else {
                    let (n10, n100) = (n % 10, n % 100);
                    if n10 >= 2 && n10 <= 4 && !(n100 >= 12 && n100 <= 14) {
                        few
                    } else {
                        many
                    }
                }
            }
            Self::OneOther => {
                if n == 1 {
                    one
                } else {
                    many
                }
            }
            Self::OneZeroOther => {
                if n <= 1 {
                    one
                } else {
                    many
                }
            }
            Self::Invariant => many,
        }
    }
}

/// Container storing one value for every supported [`Locale`] (`rustc`-checked
/// when new locales are added).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PerLocale<T> {
    pub ru: T,
    pub en: T,
}

impl<T> PerLocale<T> {
    /// Builds an entry for each supported [`Locale`].
    pub fn init(mut f: impl FnMut(Locale) -> T) -> Self {
        Self { ru: f(Locale::Ru), en: f(Locale::En) }
    }

    /// Returns the entry for `loc`.
    pub const fn get(&self, loc: Locale) -> &T {
        match loc {
            Locale::Ru => &self.ru,
            Locale::En => &self.en,
        }
    }

    /// Returns a mutable reference to the entry for `loc`.
    pub fn get_mut(&mut self, loc: Locale) -> &mut T {
        match loc {
            Locale::Ru => &mut self.ru,
            Locale::En => &mut self.en,
        }
    }
}

/// Static plural noun forms for [`plural_forms`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluralForms {
    pub one: &'static str,
    pub few: &'static str,
    pub many: &'static str,
}

/// Selects the plural form of `n` from `forms` under `loc`.
pub const fn plural_forms(loc: Locale, n: u64, forms: &PluralForms) -> &'static str {
    loc.plural_rule().select(n, forms.one, forms.few, forms.many)
}

/// Selects the plural form of `n` under `loc`.
pub const fn plural<'a>(loc: Locale, n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    loc.plural_rule().select(n, one, few, many)
}

static CURRENT: AtomicU8 = AtomicU8::new(Locale::Ru as u8);
static SYSTEM_LOCALE: AtomicU8 = AtomicU8::new(u8::MAX);
static SYSTEM_OVERRIDE: AtomicU8 = AtomicU8::new(u8::MAX);
static SYSTEM_DETECT_LOCK: Mutex<()> = Mutex::new(());

/// Active process locale, read lock-free with `Ordering::Relaxed`.
#[inline]
pub fn locale() -> Locale {
    Locale::from_u8(CURRENT.load(Ordering::Relaxed))
}

/// Sets the active process locale directly (useful for startup and tests).
pub fn set_locale(loc: Locale) {
    CURRENT.store(loc.to_u8(), Ordering::Relaxed);
}

/// Resolves `pref` against [`system_locale`] and activates the resulting [`Locale`].
pub fn apply_pref(pref: LangPref) -> Locale {
    let resolved = pref.resolve_system();
    set_locale(resolved);
    resolved
}

/// Registers an OS locale tag from a platform host (e.g. Android JNI) before
/// [`apply_pref`] / [`system_locale`] runs, or updates it when the host language changes.
pub fn set_system_locale_tag(tag: impl AsRef<str>) {
    let parsed = parse_locale_tag(tag.as_ref());
    let _guard = SYSTEM_DETECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    SYSTEM_OVERRIDE.store(parsed.to_u8(), Ordering::Release);
    SYSTEM_LOCALE.store(parsed.to_u8(), Ordering::Release);
}

/// Detects and caches the operating system language cleanly (`0 ns` lock-free fast path).
pub fn system_locale() -> Locale {
    if let Some(loc) = Locale::try_from_u8(SYSTEM_LOCALE.load(Ordering::Acquire)) {
        return loc;
    }
    let _guard = SYSTEM_DETECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(loc) = Locale::try_from_u8(SYSTEM_LOCALE.load(Ordering::Relaxed)) {
        return loc;
    }
    let resolved = Locale::try_from_u8(SYSTEM_OVERRIDE.load(Ordering::Relaxed))
        .unwrap_or_else(detect_system_locale);
    SYSTEM_LOCALE.store(resolved.to_u8(), Ordering::Release);
    resolved
}

fn detect_system_locale() -> Locale {
    if let Some(tag) = env_locale_tag() {
        return parse_locale_tag(&tag);
    }
    #[cfg(windows)]
    if let Some(tag) = windows_language() {
        return parse_locale_tag(&tag);
    }
    Locale::En
}

/// Parses a POSIX or BCP-47 language tag against supported [`Locale::ALL`]
/// codes with zero heap allocations, defaulting unsupported tags to [`Locale::En`].
pub fn parse_locale_tag(tag: &str) -> Locale {
    let base = tag.trim().split(['.', '@']).next().unwrap_or("");
    let subtag = base.as_bytes();
    for loc in Locale::ALL {
        let code = loc.code().as_bytes();
        if subtag.len() >= code.len()
            && subtag[..code.len()].eq_ignore_ascii_case(code)
            && subtag.get(code.len()).is_none_or(|b| *b == b'-' || *b == b'_')
        {
            return loc;
        }
    }
    Locale::En
}

fn env_locale_tag() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.trim().is_empty()))
}

/// The Windows display language, falling back to the user locale.
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_language() -> Option<String> {
    use windows_sys::Win32::Globalization::{
        GetUserDefaultLocaleName, GetUserPreferredUILanguages, MUI_LANGUAGE_NAME,
    };

    const NAME_LENGTH: usize = 85;
    let first = |names: &[u16]| {
        let end = names.iter().position(|unit| *unit == 0)?;
        (end > 0).then(|| String::from_utf16_lossy(&names[..end]))
    };

    let mut names = [0u16; NAME_LENGTH * 8];
    let mut count = 0u32;
    let mut length = names.len() as u32;
    // SAFETY: `GetUserPreferredUILanguages` writes at most `length` UTF-16 units into `names`.
    let preferred = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, names.as_mut_ptr(), &mut length)
    };
    if preferred != 0
        && count > 0
        && let Some(name) = first(&names)
    {
        return Some(name);
    }

    let mut name = [0u16; NAME_LENGTH];
    // SAFETY: `GetUserDefaultLocaleName` writes at most `name.len()` UTF-16 units into `name`.
    let read = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    (read > 0).then(|| first(&name)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::Service;

    #[test]
    fn parses_system_tags_strictly_ru_or_en() {
        for tag in ["ru", "RU", "ru-RU", "ru_RU.UTF-8", "ru_UA@latin", " ru-KZ "] {
            assert_eq!(parse_locale_tag(tag), Locale::Ru, "{tag}");
        }
        for tag in ["", "C", "POSIX", "en", "en-US", "en_GB.UTF-8", "be-BY", "uk-UA", "kk-KZ", "de-DE", "run"] {
            assert_eq!(parse_locale_tag(tag), Locale::En, "{tag}");
        }
    }

    #[test]
    fn lang_pref_resolves_against_system() {
        assert_eq!(LangPref::Auto.resolve(Locale::Ru), Locale::Ru);
        assert_eq!(LangPref::Auto.resolve(Locale::En), Locale::En);
        assert_eq!(LangPref::Ru.resolve(Locale::En), Locale::Ru);
        assert_eq!(LangPref::En.resolve(Locale::Ru), Locale::En);
    }

    #[test]
    fn plural_rules_handle_slavic_and_english() {
        for (n, want) in [
            (0, "треков"),
            (1, "трек"),
            (2, "трека"),
            (4, "трека"),
            (5, "треков"),
            (11, "треков"),
            (12, "треков"),
            (21, "трек"),
            (22, "трека"),
            (25, "треков"),
            (101, "трек"),
            (111, "треков"),
        ] {
            assert_eq!(plural(Locale::Ru, n, "трек", "трека", "треков"), want, "ru {n}");
        }
        assert_eq!(tracks_count_in(Locale::Ru, 1), "1 трек");
        assert_eq!(tracks_count_in(Locale::Ru, 2), "2 трека");
        assert_eq!(tracks_count_in(Locale::Ru, 5), "5 треков");
        assert_eq!(tracks_count_in(Locale::En, 0), "0 tracks");
        assert_eq!(tracks_count_in(Locale::En, 1), "1 track");
        assert_eq!(tracks_count_in(Locale::En, 2), "2 tracks");
    }

    #[test]
    fn mix_titles_translate_and_parse_legacy() {
        assert_eq!(MixTitle::Daily.title_in(Locale::Ru), "Микс дня");
        assert_eq!(MixTitle::Daily.title_in(Locale::En), "Daily mix");
        assert_eq!(MixTitle::Genre(7).title_in(Locale::Ru), "Хип-хоп микс");
        assert_eq!(MixTitle::Genre(7).title_in(Locale::En), "Hip-hop mix");
        assert_eq!(MixTitle::Artist("Кино".into()).title_in(Locale::En), "Кино mix");
        assert_eq!(MixTitle::from_legacy("Микс дня"), MixTitle::Daily);
        assert_eq!(MixTitle::from_legacy("Хип-хоп микс"), MixTitle::Genre(7));
        assert_eq!(MixTitle::from_legacy("Микс: ZUTOMAYO"), MixTitle::Artist("ZUTOMAYO".into()));

        assert_eq!(MixTitle::from_legacy("Микс 90-х"), MixTitle::Era(1990));
        assert_eq!(MixTitle::Era(1990).title_in(Locale::En), "90s mix");
        assert_eq!(MixTitle::from_legacy("Микс 2010-х"), MixTitle::Era(2010));
        assert_eq!(MixTitle::Era(2010).title_in(Locale::En), "2010s mix");

        assert_eq!(MixTitle::from_legacy("Похоже на Underground River"), MixTitle::BecauseYouLiked("Underground River".into()));
        assert_eq!(MixTitle::from_legacy("Похоже на «Underground River»"), MixTitle::BecauseYouLiked("Underground River".into()));
        assert_eq!(MixTitle::BecauseYouLiked("Underground River".into()).title_in(Locale::Ru), "Похоже на Underground River");
        assert_eq!(MixTitle::BecauseYouLiked("Underground River".into()).title_in(Locale::En), "Similar to Underground River");

        let world_mix = MixTitle::World { world: Some("MADE IN ABYSS".into()), mix: WorldMixKind::Best };
        assert_eq!(world_mix.card_title_in(Locale::Ru), "Лучшее");
        assert_eq!(world_mix.card_title_in(Locale::En), "Best of");
        assert_eq!(world_mix.title_in(Locale::Ru), "MADE IN ABYSS: лучшее");
        assert_eq!(world_mix.title_in(Locale::En), "MADE IN ABYSS: Best of");
        assert_eq!(MixTitle::from_legacy("MADE IN ABYSS: лучшее"), world_mix);

        let ru_calm = MixTitle::Side { calm: true, flavor: SideMixFlavor::Russian };
        assert_eq!(ru_calm.title_in(Locale::Ru), "Русские песни потише");
        assert_eq!(ru_calm.title_in(Locale::En), "Calm Russian songs");
        assert_eq!(MixTitle::from_legacy("Русские песни потише"), ru_calm);
        assert_eq!(MixTitle::from_legacy("Бодрое без слов"), MixTitle::Side { calm: false, flavor: SideMixFlavor::Instrumental });
        assert_eq!(MixTitle::from_legacy("Погромче"), MixTitle::Side { calm: false, flavor: SideMixFlavor::Plain });
    }

    #[test]
    fn queue_origin_formats_and_computes_exact_link_ranges() {
        let album = QueueOrigin::Album("A «B» C".into());
        let (ru_line, ru_link) = album.header_in(Locale::Ru, true);
        assert_eq!(ru_line, "Играет из альбома «A «B» C»");
        assert_eq!(&ru_line[ru_link.unwrap()], "A «B» C");

        let (en_line, en_link) = album.header_in(Locale::En, true);
        assert_eq!(en_line, "Playing from album “A «B» C”");
        assert_eq!(&en_line[en_link.unwrap()], "A «B» C");

        let liked = QueueOrigin::Liked;
        let (ru_liked, ru_liked_link) = liked.header_in(Locale::Ru, true);
        assert_eq!(ru_liked, "Играет из плейлиста «Любимые»");
        assert_eq!(&ru_liked[ru_liked_link.unwrap()], "Любимые");
        let (en_liked, en_liked_link) = liked.header_in(Locale::En, true);
        assert_eq!(en_liked, "Playing from playlist “Liked Songs”");
        assert_eq!(&en_liked[en_liked_link.unwrap()], "Liked Songs");

        let liked_radio = QueueOrigin::Radio(RadioOrigin::Liked);
        assert_eq!(liked_radio.header_in(Locale::En, false).0, "Playing from playlist radio “Liked Songs”");
        assert_eq!(liked_radio.save_name_in(Locale::En).as_deref(), Some("Liked Songs"));
        assert_eq!(QueueOrigin::from_legacy(liked_radio.legacy_text().as_deref()), liked_radio);

        let artist = QueueOrigin::Artist("Кино".into());
        assert_eq!(QueueOrigin::from_legacy(artist.legacy_text().as_deref()), artist);

        let search = QueueOrigin::Search { service: Some(Service::Spotify) };
        assert_eq!(search.header_in(Locale::Ru, false), ("Играет из поиска Spotify".into(), None));
        assert_eq!(search.header_in(Locale::En, false), ("Playing from Spotify search".into(), None));

        assert_eq!(QueueOrigin::from_legacy(Some("поиска Spotify: лёд")), search);
        assert_eq!(QueueOrigin::from_legacy(Some("библиотеки")), QueueOrigin::Library);
        assert_eq!(QueueOrigin::from_legacy(Some("альбома «Группа крови»")), QueueOrigin::Album("Группа крови".into()));
    }

    #[test]
    fn lang_pref_labels_use_native_endonyms_and_resolve_system() {
        assert_eq!(LangPref::Auto.label_in(Locale::Ru), "Системный");
        assert_eq!(LangPref::Auto.label_in(Locale::En), "System");
        for loc in Locale::ALL {
            assert_eq!(LangPref::Ru.label_in(loc), "Русский");
            assert_eq!(LangPref::En.label_in(loc), "English (beta)");
            for pref in LangPref::ALL {
                assert_eq!(lang_pref_label_in(loc, pref), pref.label_in(loc));
            }
        }
        assert_eq!(LangPref::Ru.resolve_system(), Locale::Ru);
        assert_eq!(LangPref::En.resolve_system(), Locale::En);
        assert_eq!(LangPref::Auto.resolve_system(), system_locale());
    }

    #[test]
    fn plural_rules_cover_all_five_variants_and_common_forms() {
        assert_eq!(Locale::Ru.plural_rule(), PluralRule::EastSlavic);
        assert_eq!(Locale::En.plural_rule(), PluralRule::OneOther);

        for (n, want) in [
            (0, "many"),
            (1, "one"),
            (2, "few"),
            (4, "few"),
            (5, "many"),
            (11, "many"),
            (12, "many"),
            (20, "many"),
            (21, "one"),
            (22, "few"),
            (24, "few"),
            (25, "many"),
            (101, "one"),
            (111, "many"),
            (112, "many"),
        ] {
            assert_eq!(PluralRule::EastSlavic.select(n, "one", "few", "many"), want, "east {n}");
        }

        for (n, want) in [
            (0, "many"),
            (1, "one"),
            (2, "few"),
            (4, "few"),
            (5, "many"),
            (11, "many"),
            (12, "many"),
            (14, "many"),
            (21, "many"),
            (22, "few"),
            (24, "few"),
            (25, "many"),
            (101, "many"),
            (112, "many"),
        ] {
            assert_eq!(PluralRule::WestSlavic.select(n, "one", "few", "many"), want, "west {n}");
        }

        for (n, want) in [(0, "many"), (1, "one"), (2, "many"), (21, "many")] {
            assert_eq!(PluralRule::OneOther.select(n, "one", "few", "many"), want, "one_other {n}");
        }

        for (n, want) in [(0, "one"), (1, "one"), (2, "many"), (21, "many")] {
            assert_eq!(PluralRule::OneZeroOther.select(n, "one", "few", "many"), want, "one_zero {n}");
        }

        for n in [0, 1, 2, 5, 21, 100] {
            assert_eq!(PluralRule::Invariant.select(n, "one", "few", "many"), "many", "invariant {n}");
        }

        assert_eq!(plural_forms(Locale::Ru, 1, &common_in(Locale::Ru).artists), "исполнитель");
        assert_eq!(plural_forms(Locale::Ru, 3, &common_in(Locale::Ru).artists), "исполнителя");
        assert_eq!(plural_forms(Locale::Ru, 5, &common_in(Locale::Ru).artists), "исполнителей");
        assert_eq!(plural_forms(Locale::En, 1, &common_in(Locale::En).artists), "artist");
        assert_eq!(plural_forms(Locale::En, 2, &common_in(Locale::En).artists), "artists");
        assert_eq!(plural_forms(Locale::Ru, 21, &common_in(Locale::Ru).tracks), "трек");
        assert_eq!(plural_forms(Locale::En, 0, &common_in(Locale::En).tracks), "tracks");
    }

    #[test]
    fn radio_title_translates_headers_origins_and_legacy() {
        assert_eq!(RadioTitle::default(), RadioTitle::Wave);

        assert_eq!(RadioTitle::Wave.title_in(Locale::Ru), "Моя волна");
        assert_eq!(RadioTitle::Wave.title_in(Locale::En), "My Wave");
        assert_eq!(RadioTitle::Wave.queue_header_in(Locale::Ru), "Микс · Моя волна");
        assert_eq!(RadioTitle::Wave.queue_header_in(Locale::En), "Mix · My Wave");
        assert_eq!(RadioTitle::Wave.seed_name(), None);
        assert_eq!(RadioTitle::Wave.to_queue_origin(), QueueOrigin::Wave);

        assert_eq!(RadioTitle::Discover.title_in(Locale::Ru), "Новое для тебя");
        assert_eq!(RadioTitle::Discover.title_in(Locale::En), "Discover");
        assert_eq!(RadioTitle::Discover.queue_header_in(Locale::En), "Mix · Discover");
        assert_eq!(RadioTitle::Discover.seed_name(), None);
        assert_eq!(RadioTitle::Discover.to_queue_origin(), QueueOrigin::Discover);

        let seed = RadioTitle::Seed("Кино".into());
        assert_eq!(seed.title_in(Locale::Ru), "Радио · Кино");
        assert_eq!(seed.title_in(Locale::En), "Radio · Кино");
        assert_eq!(seed.queue_header_in(Locale::Ru), "Радио · Кино");
        assert_eq!(seed.queue_header_in(Locale::En), "Radio · Кино");
        assert_eq!(seed.seed_name(), Some("Кино"));
        assert_eq!(seed.to_queue_origin(), QueueOrigin::Radio(RadioOrigin::Named("Кино".into())));

        let album = RadioTitle::Album("Группа крови".into());
        assert_eq!(album.title_in(Locale::Ru), "Радио по альбому «Группа крови»");
        assert_eq!(album.title_in(Locale::En), "Album radio “Группа крови”");
        assert_eq!(album.queue_header_in(Locale::En), "Album radio “Группа крови”");
        assert_eq!(album.seed_name(), Some("Группа крови"));
        assert_eq!(album.to_queue_origin(), QueueOrigin::Radio(RadioOrigin::Album("Группа крови".into())));

        let pl = RadioTitle::Playlist { name: "Избранное".into(), liked: false };
        assert_eq!(pl.title_in(Locale::Ru), "Радио по плейлисту «Избранное»");
        assert_eq!(pl.title_in(Locale::En), "Playlist radio “Избранное”");
        assert_eq!(pl.queue_header_in(Locale::En), "Playlist radio “Избранное”");
        assert_eq!(pl.seed_name(), None);
        assert_eq!(pl.to_queue_origin(), QueueOrigin::Radio(RadioOrigin::Playlist("Избранное".into())));

        let liked = RadioTitle::Playlist { name: String::new(), liked: true };
        assert_eq!(liked.title_in(Locale::Ru), "Радио по плейлисту «Любимые»");
        assert_eq!(liked.title_in(Locale::En), "Playlist radio “Liked Songs”");
        assert_eq!(liked.queue_header_in(Locale::En), "Playlist radio “Liked Songs”");
        assert_eq!(liked.seed_name(), None);
        assert_eq!(liked.to_queue_origin(), QueueOrigin::Radio(RadioOrigin::Liked));

        let auto = RadioTitle::Autoplay("Звезда".into());
        assert_eq!(auto.title_in(Locale::Ru), "Похожее на «Звезда»");
        assert_eq!(auto.title_in(Locale::En), "Similar to “Звезда”");
        assert_eq!(auto.queue_header_in(Locale::Ru), "Микс · Похожее на «Звезда»");
        assert_eq!(auto.queue_header_in(Locale::En), "Mix · Similar to “Звезда”");
        assert_eq!(auto.seed_name(), Some("Звезда"));
        assert_eq!(auto.to_queue_origin(), QueueOrigin::Mix(MixTitle::BecauseYouLiked("Звезда".into())));

        let mix = RadioTitle::Mix(MixTitle::Genre(7));
        assert_eq!(mix.title_in(Locale::Ru), "Хип-хоп микс");
        assert_eq!(mix.title_in(Locale::En), "Hip-hop mix");
        assert_eq!(mix.queue_header_in(Locale::Ru), "Микс · Хип-хоп микс");
        assert_eq!(mix.queue_header_in(Locale::En), "Mix · Hip-hop mix");
        assert_eq!(mix.seed_name(), None);
        assert_eq!(mix.to_queue_origin(), QueueOrigin::Mix(MixTitle::Genre(7)));

        assert_eq!(RadioTitle::from_legacy("Моя волна"), RadioTitle::Wave);
        assert_eq!(RadioTitle::from_legacy("Новое для тебя"), RadioTitle::Discover);
        assert_eq!(RadioTitle::from_legacy("Радио · Кино"), seed);
        assert_eq!(RadioTitle::from_legacy("Радио по альбому «Группа крови»"), album);
        assert_eq!(RadioTitle::from_legacy("Радио по плейлисту «Любимые»"), liked);
        assert_eq!(RadioTitle::from_legacy("Радио по плейлисту «Избранное»"), pl);
        assert_eq!(RadioTitle::from_legacy("Похожее на «Звезда»"), auto);
        assert_eq!(
            RadioTitle::from_legacy("Похоже на Underground River"),
            RadioTitle::Mix(MixTitle::BecauseYouLiked("Underground River".into()))
        );
        assert_eq!(
            RadioTitle::from_legacy("Похожее на «Underground River»"),
            RadioTitle::Autoplay("Underground River".into())
        );
        assert_eq!(RadioTitle::from_legacy("Спокойное"), RadioTitle::Mix(MixTitle::Mood(MoodKind::Calm)));

        let round: RadioTitle = serde_json::from_str(&serde_json::to_string(&liked).unwrap()).unwrap();
        assert_eq!(round, liked);
    }

    #[test]
    fn player_and_error_strings_cover_stream_and_spotify() {
        assert_eq!(player_in(Locale::Ru).fact_stream, "Поток");
        assert_eq!(player_in(Locale::En).fact_stream, "Streaming");
        assert_eq!(errors_in(Locale::Ru).spotify_need_premium, "для звука из Spotify нужен Premium");
        assert_eq!(
            errors_in(Locale::Ru).spotify_enable_playback,
            "для звука из Spotify включи воспроизведение в настройках"
        );
        assert_eq!(errors_in(Locale::Ru).spotify_unavailable, "трек недоступен");
        assert!(!errors_in(Locale::En).spotify_need_premium.is_empty());
        assert!(!errors_in(Locale::En).spotify_enable_playback.is_empty());
        assert!(!errors_in(Locale::En).spotify_unavailable.is_empty());
        assert_eq!(lyrics_in(Locale::En).gpu, "GPU");
        assert_eq!(nav_in(Locale::En).liked, "Liked Songs");
        assert_eq!(errors_in(Locale::En).find_song, "Find a song…");
        assert_eq!(errors_in(Locale::En).sign_in, "Sign in");
        assert_eq!(errors_in(Locale::En).playback_failed, "Could not play the track");
        assert_eq!(errors_in(Locale::En).no_file_for_track, "No file for this track");
        assert_eq!(errors_in(Locale::En).audio_output_failed, "Could not open audio output device");
        assert_eq!(errors_in(Locale::En).not_on_youtube, "Not on YouTube Music");
        assert_eq!(
            ErrorStrings::resolve_rate_limited_in(Locale::En, "Song", "YouTube Music", Some("14:05")),
            "Song: YouTube Music is asking you to wait until 14:05 — try playing this track later"
        );
        assert_eq!(
            ErrorStrings::resolve_rate_limited_in(Locale::Ru, "Song", "YouTube Music", None),
            "Song: YouTube Music просит подождать — включи трек позже"
        );
        assert_eq!(
            ErrorStrings::pause_will_resume_in(Locale::En, "YouTube Music", None),
            "YouTube Music needs more time — playback will resume automatically"
        );
        assert_eq!(
            ErrorStrings::pause_will_resume_in(Locale::Ru, "YouTube Music", Some("14:05")),
            "YouTube Music просит подождать до 14:05 — продолжу сам"
        );
        assert_eq!(
            ErrorStrings::sync_failed_in(Locale::En, "Unavailable"),
            "Sync failed: Unavailable"
        );
        assert_eq!(
            ErrorStrings::sync_update_failed_in(Locale::En, "Sign in required"),
            "Could not update the playlist on the service: Sign in required"
        );
        assert_eq!(
            ErrorStrings::sync_create_failed_in(Locale::En, "Spotify", "Unavailable"),
            "Could not create a playlist on Spotify: Unavailable"
        );
        assert_eq!(
            ErrorStrings::mesh_forget_failed_in(Locale::En, None, "offline"),
            "Could not forget “device”: offline"
        );
        assert_eq!(
            ErrorStrings::masters_move_failed_in(Locale::En, "refused"),
            "Could not move FLAC: refused"
        );
        assert_eq!(
            ErrorStrings::masters_move_failed_in(Locale::Ru, "отказ"),
            "Не удалось перенести FLAC: отказ"
        );
        assert_eq!(
            ErrorStrings::masters_named_in_albums_in(Locale::En, "C:\\Music", 2),
            "C:\\Music has FLAC masters stored inside album folders (2)"
        );
        assert_eq!(
            ErrorStrings::hash_verification_failed_in(Locale::En, "C:\\dst.flac", "exp", Some(12), "act", Some(10)),
            "hash verification failed for C:\\dst.flac: expected exp (12 B), got act (10 B)"
        );
        assert_eq!(
            ErrorStrings::sidecar_verification_failed_in(Locale::En, "C:\\dst.json"),
            "metadata verification failed for C:\\dst.json"
        );
        assert_eq!(ErrorStrings::service_not_connected_in(Locale::En, "Spotify"), "Spotify not connected");
        assert_eq!(ErrorStrings::service_disabled_in(Locale::Ru, "Deezer"), "Deezer выключен");
        assert_eq!(
            ErrorStrings::song_not_found_on_in(Locale::En, "Track", "YouTube Music"),
            "Track: not found on YouTube Music"
        );
        assert_eq!(fmt_khz_in(Locale::Ru, 44_100), "44,1 кГц");
        assert_eq!(fmt_khz_in(Locale::En, 44_100), "44.1 kHz");
        assert_eq!(fmt_mb_in(Locale::Ru, 26_733_824), "25,5 МБ");
        assert_eq!(fmt_mb_in(Locale::En, 26_733_824), "25.5 MB");
        assert_eq!(sound_facts_in(Locale::En).bitrate, "Bitrate");

        let localized = MixTitle::Localized(PerLocale::init(|loc| match loc {
            Locale::Ru => "Локальный".into(),
            Locale::En => "Localized".into(),
        }));
        assert_eq!(localized.title_in(Locale::Ru), "Локальный");
        assert_eq!(localized.title_in(Locale::En), "Localized");
        for loc in Locale::ALL {
            for id in 1..=23u8 {
                assert!(genre_family_title_in(loc, id).is_some(), "{loc:?} genre {id}");
            }
            assert_eq!(genre_family_title_in(loc, 0), None);
        }
    }

    #[test]
    fn locale_u8_and_tags_round_trip() {
        for loc in Locale::ALL {
            assert_eq!(Locale::try_from_u8(loc.to_u8()), Some(loc));
            assert_eq!(Locale::from_u8(loc.to_u8()), loc);
            assert_eq!(parse_locale_tag(loc.code()), loc);
        }
        assert_eq!(Locale::try_from_u8(u8::MAX), None);
        assert_eq!(parse_locale_tag("ru_RU.UTF-8"), Locale::Ru);
        assert_eq!(parse_locale_tag("EN-us@latin"), Locale::En);
        assert_eq!(parse_locale_tag("日本語"), Locale::En);
        assert_eq!(parse_locale_tag("中文-TW.UTF-8"), Locale::En);
        assert_eq!(parse_locale_tag("ру"), Locale::En);
        assert_eq!(parse_locale_tag(""), Locale::En);
    }
}
