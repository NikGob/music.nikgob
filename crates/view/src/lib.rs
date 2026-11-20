//! The view model every design system draws from.
//!
//! The runtime (library, player, import, downloads) fills a [`ViewState`];
//! a design system reads it each frame and answers with [`Command`]s. Neither
//! side knows how the other works: MD3E, Nothing and Liquid Glass are
//! different layouts over the same state.

use std::path::PathBuf;

mod api;
mod browse;
mod command;
mod connect;
mod credit;
mod home;
mod library;
mod mesh;
pub mod dev;
pub mod first_run;
mod player;
mod player_prefs;
mod prefs;
mod prefs_edit;
pub mod presets;
mod recs;
mod sleep;
mod sound;
mod storage;
mod streams;
mod transfer;

pub use nikgob_core::i18n::{
    LangPref, Locale, MixTitle, PerLocale, QueueOrigin, RadioOrigin, RadioTitle, SideMixFlavor, locale, view_in,
};
pub use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource, LyricsSync};
pub use nikgob_core::model::AlbumMode;
pub use nikgob_core::model::{
    ArtistId, LikeTarget, PlaylistId, ReleaseId, ReleaseType, SourceKind, TrackId,
};
pub use api::*;
pub use browse::*;
pub use command::*;
pub use connect::*;
pub use credit::*;
pub use home::*;
pub use library::*;
pub use mesh::*;
pub use player::*;
pub use player_prefs::*;
pub use prefs::*;
pub use recs::*;
pub use sleep::*;
pub use sound::*;
pub use storage::*;
pub use streams::*;
pub use transfer::*;

/// A cover image on disk plus the palette seed computed at import.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Cover {
    pub path: PathBuf,
    /// ARGB seed for the colour scheme, if the cover is colourful enough.
    pub seed: Option<u32>,
    /// A tiny (16×16) downscale of the cover; stretched with linear
    /// filtering it is the blurred ambient background.
    pub ambient: Option<PathBuf>,
}

impl Cover {
    /// A cover of a streaming service, drawn from its `https://` url (the
    /// app's web image loader fetches it; a placeholder while it cannot).
    pub fn web(url: impl Into<String>) -> Self {
        Self {
            path: PathBuf::from(url.into()),
            seed: None,
            ambient: None,
        }
    }

    /// The url of a [`Self::web`] cover.
    pub fn web_url(&self) -> Option<&str> {
        self.path
            .to_str()
            .filter(|p| p.starts_with("https://") || p.starts_with("http://"))
    }

    /// The cover file on disk (`None` for a web cover).
    pub fn file(&self) -> Option<&std::path::Path> {
        self.web_url().is_none().then_some(self.path.as_path())
    }

    /// The egui image URI for this file (a web cover: its url).
    pub fn uri(&self) -> String {
        match self.web_url() {
            Some(url) => url.to_owned(),
            None => file_uri(&self.path),
        }
    }

    pub fn ambient_uri(&self) -> Option<String> {
        self.ambient.as_deref().map(file_uri)
    }
}

/// `file://` URI egui's file loader understands (forward slashes on Windows).
pub fn file_uri(path: &std::path::Path) -> String {
    let p = path.display().to_string().replace('\\', "/");
    // egui_extras wants `file:///C:/...` on Windows (empty host).
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// Live audio levels for ambient effects, read by the UI only while visible.
pub trait Levels {
    /// Bass energy 20–150 Hz, smoothed, 0..1.
    fn bass(&self) -> f32;
    /// Log-spaced spectrum bands, 0..1 each; fills `out`.
    fn bands(&self, out: &mut [f32]);
}

/// No audio analysis available.
pub struct NoLevels;

impl Levels for NoLevels {
    fn bass(&self) -> f32 {
        0.0
    }
    fn bands(&self, out: &mut [f32]) {
        out.fill(0.0);
    }
}

#[cfg(test)]
mod toast_tests {
    use std::time::Instant;

    use super::{LangPref, Locale, Prefs, Toast, ToastKind};

    #[test]
    fn toast_kind_defaults_to_info_and_variants_are_distinct() {
        assert_eq!(ToastKind::default(), ToastKind::Info);
        let kinds = [ToastKind::Info, ToastKind::Success, ToastKind::Working, ToastKind::Error];
        for (i, a) in kinds.iter().enumerate() {
            for (j, b) in kinds.iter().enumerate() {
                assert_eq!(a == b, i == j);
            }
        }
        let toast = Toast {
            id: 1,
            text: "Saved".into(),
            action: None,
            open: None,
            at: Instant::now(),
            kind: ToastKind::Success,
            long: false,
            again: 0,
        };
        assert_eq!(toast.kind, ToastKind::Success);
        assert!(!toast.long);
    }

    #[test]
    fn prefs_resolved_locale_respects_explicit_language() {
        let ru = Prefs { language: LangPref::Ru, ..Prefs::default() };
        let en = Prefs { language: LangPref::En, ..Prefs::default() };
        assert_eq!(ru.resolved_locale(), Locale::Ru);
        assert_eq!(en.resolved_locale(), Locale::En);
    }
}


#[cfg(test)]
mod context_tests {
    use super::{Locale, PlayerView, QueueOrigin};

    fn line(c: Option<&str>) -> String {
        PlayerView { origin: QueueOrigin::from_legacy(c), ..PlayerView::default() }
            .context_header(Locale::Ru)
            .0
    }

    #[test]
    fn contexts_read_without_quotes_in_quotes() {
        assert_eq!(line(None), "Сейчас играет");
        assert_eq!(line(Some("Библиотека")), "Играет из «Библиотека»");
        assert_eq!(line(Some("библиотеки")), "Играет из библиотеки");
        assert_eq!(line(Some("альбома «Группа крови»")), "Играет из альбома «Группа крови»");
        assert_eq!(line(Some("плейлиста «Вечер»")), "Играет из плейлиста «Вечер»");
        assert_eq!(line(Some("поиска")), "Играет из поиска");
        assert_eq!(line(Some("поиска Spotify")), "Играет из поиска Spotify");
        assert_eq!(line(Some("Кино")), "Играет из «Кино»");
        // Saved with the typed query: the query never shows.
        assert_eq!(line(Some("поиска Spotify: лай мне все")), "Играет из поиска Spotify");
        assert_eq!(line(Some("поиска: лай мне все")), "Играет из поиска");
        assert_eq!(line(Some("Spotify · «лёд»")), "Играет из поиска Spotify");
        assert_eq!(line(Some("Поиск «лёд»")), "Играет из поиска");
        for c in ["Spotify · «лёд»", "Поиск «лёд»", "альбома «X»", "Кино"] {
            assert!(line(Some(c)).matches('«').count() <= 1, "{c}");
        }
        assert_eq!(
            PlayerView { origin: QueueOrigin::Album("Blood Type".into()), ..PlayerView::default() }
                .context_header(Locale::En)
                .0,
            "Playing from album “Blood Type”"
        );
    }

    #[test]
    fn the_name_of_a_page_is_the_link_of_the_header() {
        let with = |origin: QueueOrigin, page: Option<crate::Route>, loc: Locale| {
            let p = PlayerView { origin, context_page: page, ..PlayerView::default() };
            let (l, r) = p.context_header(loc);
            r.map(|r| l[r].to_owned())
        };
        let album = Some(crate::Route::Release(crate::ReleaseId::new()));
        assert_eq!(with(QueueOrigin::Album("Группа крови".into()), album.clone(), Locale::Ru).as_deref(), Some("Группа крови"));
        assert_eq!(with(QueueOrigin::Album("A «B»".into()), album.clone(), Locale::En).as_deref(), Some("A «B»"));
        assert_eq!(with(QueueOrigin::Artist("Кино".into()), Some(crate::Route::Artist(crate::ArtistId::new())), Locale::Ru).as_deref(), Some("Кино"));
        assert_eq!(with(QueueOrigin::Liked, Some(crate::Route::Playlist(crate::LIKED_PLAYLIST)), Locale::En).as_deref(), Some("Liked Songs"));
        // No page (a search, a queue of picked songs), or nothing quoted.
        assert_eq!(with(QueueOrigin::Album("Группа крови".into()), None, Locale::Ru), None);
        assert_eq!(with(QueueOrigin::Search { service: None }, album.clone(), Locale::Ru), None);
        assert_eq!(with(QueueOrigin::Library, album, Locale::Ru), None);
    }
}

#[cfg(test)]
mod release_day_tests {
    use super::ReleaseDay;

    #[test]
    fn dates_parse_to_the_day_or_the_month() {
        let day = |y, m, d| Some(ReleaseDay { year: y, month: m, day: d });
        assert_eq!(ReleaseDay::parse("2019-05-17", Some(2019)), day(2019, 5, Some(17)));
        assert_eq!(ReleaseDay::parse(" 2019-05-17T00:00:00Z ", None), day(2019, 5, Some(17)));
        assert_eq!(ReleaseDay::parse("2019/5/7", None), day(2019, 5, Some(7)));
        assert_eq!(ReleaseDay::parse("2019-05", Some(2019)), day(2019, 5, None));
        // A bare year says nothing the card does not.
        assert_eq!(ReleaseDay::parse("2019", Some(2019)), None);
        // A recording's date of another year is not the album's.
        assert_eq!(ReleaseDay::parse("2003-02-01", Some(2019)), None);
        for bad in ["", "19-05-17", "2019-13-01", "2019-05-32", "2019-05-17-1", "май 2019", "2019-05-00"] {
            assert_eq!(ReleaseDay::parse(bad, None), None, "{bad}");
        }
    }
}

#[cfg(test)]
mod settings_v2_tests {
    use super::*;
    use crate::presets::Area;

    #[test]
    fn home_sections_are_all_there_once_in_the_given_order() {
        let p = Prefs::default();
        assert_eq!(p.home_order().iter().map(|s| s.key).collect::<Vec<_>>(), HomeSection::ALL);
        assert!(HomeSection::ALL.iter().all(|&s| p.home_section_on(s)));
        let mut q = p.clone();
        PrefPatch::HomeSections(vec![
            HomeSectionSwitch { key: HomeSection::Releases, on: false },
            HomeSectionSwitch { key: HomeSection::Releases, on: true },
            HomeSectionSwitch { key: HomeSection::Mixes, on: true },
        ])
        .apply(&mut q);
        let keys: Vec<_> = q.home_sections.iter().map(|s| s.key).collect();
        // The given ones keep their order; a missing one sits after the
        // default ones before it (Быстрый доступ has none: first).
        use HomeSection as S;
        assert_eq!(keys, [S::Shortcuts, S::Releases, S::Mixes, S::ListenAgain, S::OfDay, S::FromLibrary, S::New]);
        // A list saved with the row there no longer is reads without it.
        let old = [HomeSectionSwitch { key: S::Daily, on: false }, HomeSectionSwitch { key: S::Mixes, on: true }];
        assert!(HomeSectionSwitch::complete(&old).iter().all(|s| s.key != S::Daily));
        assert_eq!(HomeSectionSwitch::complete(&old).len(), HomeSection::ALL.len());
        assert!(!q.home_section_on(S::Releases), "the first of a section counts");
        assert!(q.home_section_on(S::Shortcuts));
    }

    #[test]
    fn preset_patches_apply_and_come_back() {
        let mut p = Prefs { fps_limit: 90, ..Prefs::default() };
        PrefPatch::Preset(Area::Load, 1).apply(&mut p);
        assert_eq!((p.fps_limit, Area::Load.current(&p)), (120, Some(1)));
        PrefPatch::PresetCustom(Area::Load).apply(&mut p);
        assert_eq!(p.fps_limit, 90);
        // «Свои» without kept values changes nothing.
        let mut f = Prefs::first_start();
        PrefPatch::PresetCustom(Area::Track).apply(&mut f);
        assert_eq!(f, Prefs::first_start());
        PrefPatch::SettingsDeep(true).apply(&mut f);
        assert!(f.settings_deep);
    }

    #[test]
    fn the_new_knobs_default_to_what_the_code_drew() {
        for p in [Prefs::default(), Prefs::first_start()] {
            assert_eq!((p.cover_spin_s, p.cover_pulse_strength), (48.0, 0.08));
            assert!(!p.settings_deep && p.custom_presets.is_empty() && p.home_sections.is_empty());
            assert!(COVER_SPIN_RANGE.contains(&p.cover_spin_s));
            assert!(COVER_PULSE_RANGE.contains(&p.cover_pulse_strength));
        }
    }

    /// «Масштаб интерфейса»: 100 % by default, kept on a step of 10 %
    /// between 80 and 150 %, the keys a step at a time; the patch writes
    /// that field alone.
    #[test]
    fn the_interface_scale_stays_on_its_steps() {
        for p in [Prefs::default(), Prefs::first_start()] {
            assert_eq!(p.ui_scale, 1.0);
        }
        assert_eq!(ui_scale(1.0), 1.0);
        assert_eq!(ui_scale(1.13), 1.1);
        assert_eq!(ui_scale(0.5), 0.8);
        assert_eq!(ui_scale(3.0), 1.5);
        assert_eq!(ui_scale(f32::NAN), 1.0);
        assert_eq!(ui_scale_step(1.0, 1), 1.1);
        assert_eq!(ui_scale_step(1.0, -1), 0.9);
        assert_eq!(ui_scale_step(1.4, 1), 1.5);
        assert_eq!(ui_scale_step(1.5, 1), 1.5, "not past the end");
        assert_eq!(ui_scale_step(0.8, -1), 0.8);
        assert_eq!(ui_scale_step(0.9, -3), 0.8);
        let mut s = 0.8;
        for want in [0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5] {
            s = ui_scale_step(s, 1);
            assert_eq!(s, want);
        }
        let mut p = Prefs::default();
        let before = p.clone();
        PrefPatch::UiScale(1.26).apply(&mut p);
        assert_eq!(p.ui_scale, 1.3);
        assert_eq!(Prefs { ui_scale: before.ui_scale, ..p.clone() }, before, "nothing else changes");
        // The settings page's copy: only the scale is laid over.
        let mut now = Prefs { rail_compact: true, ..Prefs::default() };
        let after = Prefs { ui_scale: 1.2, ..Prefs::default() };
        now.merge_edit(&Prefs::default(), &after);
        assert!(now.rail_compact);
        assert_eq!(now.ui_scale, 1.2);
    }
}

#[cfg(test)]
mod track_menu_tests {
    use serde::Deserialize;
    use serde::de::value::{Error, MapDeserializer};

    use super::{Prefs, TrackMenuItem as I, TrackMenuPlace as P, TrackMenuSlot};

    #[test]
    fn the_menu_is_as_it_was_until_an_item_is_moved() {
        let mut p = Prefs::default();
        assert!(p.track_menu.is_empty() && Prefs::first_start().track_menu.is_empty());
        for item in I::ALL {
            assert_eq!(p.track_menu_place(item), item.default_place());
        }
        // «Скачать» in the menu itself, «Источник и качество» in «Файл».
        assert_eq!(
            (I::Info.default_place(), I::Tags.default_place(), I::Download.default_place(), I::Share.default_place()),
            (P::File, P::File, P::Menu, P::Menu)
        );
        p.set_track_menu_place(I::Share, P::File);
        p.set_track_menu_place(I::Tags, P::Menu);
        p.set_track_menu_place(I::Ban, P::Hidden);
        assert_eq!((p.track_menu_place(I::Share), p.track_menu_place(I::Tags), p.track_menu_place(I::Ban)), (P::File, P::Menu, P::Hidden));
        // Back at its default place an item is not kept.
        p.set_track_menu_place(I::Share, P::Menu);
        p.set_track_menu_place(I::Tags, P::File);
        assert_eq!(p.track_menu, [TrackMenuSlot { item: I::Ban, place: P::Hidden }]);
    }

    /// A layout saved while «Выбрать раздачу…» was part of «Найти
    /// качество лучше» (one item, `torrents`) keeps working: the new item
    /// goes where that one went until it is placed itself, and placing
    /// either later leaves the other where it stood.
    #[test]
    fn a_layout_saved_before_the_split_places_both_torrent_items() {
        let mut p = Prefs { track_menu: vec![TrackMenuSlot { item: I::Torrents, place: P::Hidden }], ..Prefs::default() };
        assert_eq!((p.track_menu_place(I::Torrents), p.track_menu_place(I::PickRelease)), (P::Hidden, P::Hidden));
        p.set_track_menu_place(I::Torrents, P::Menu);
        assert_eq!((p.track_menu_place(I::Torrents), p.track_menu_place(I::PickRelease)), (P::Menu, P::Hidden));
        p.set_track_menu_place(I::PickRelease, P::File);
        assert_eq!((p.track_menu_place(I::Torrents), p.track_menu_place(I::PickRelease)), (P::Menu, P::File));
        // Nothing saved: both in «Файл»; placing «Найти качество лучше»
        // after the split leaves «Выбрать раздачу…» where it is.
        let mut p = Prefs::default();
        assert_eq!((p.track_menu_place(I::Torrents), p.track_menu_place(I::PickRelease)), (P::File, P::File));
        p.set_track_menu_place(I::Torrents, P::Menu);
        assert_eq!((p.track_menu_place(I::Torrents), p.track_menu_place(I::PickRelease)), (P::Menu, P::File));
        p.set_track_menu_place(I::Torrents, P::Hidden);
        assert_eq!((p.track_menu_place(I::Torrents), p.track_menu_place(I::PickRelease)), (P::Hidden, P::File));
    }

    /// Prefs saved before «Меню трека» have no such field: the default.
    #[test]
    fn old_prefs_load_with_the_default_menu() {
        let old = MapDeserializer::<_, Error>::new([("tooltips", true)].into_iter());
        let p = Prefs::deserialize(old).unwrap();
        assert!(p.tooltips && p.track_menu.is_empty());
        assert_eq!(p, Prefs { tooltips: true, ..Prefs::default() });
    }
}
