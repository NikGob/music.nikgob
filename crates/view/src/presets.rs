//! Presets of the settings: a few named sets of values for a group of
//! preferences («Экран трека → Строго · Живо · Максимум»).
//!
//! A preset is never stored: which one is chosen is worked out from the
//! fields themselves ([`Area::current`]), and none matching is «Свои». A
//! preset owns only the fields of its table ([`Area::fields`]); the other
//! rows of its group are fine tuning and never make it «Свои». Before a
//! preset is applied over values of the user's own, those values are kept
//! ([`Prefs::custom_presets`]) so «Свои» brings them back ([`Area::custom`]).
//! Every function here is pure: the settings can show what a preset would
//! do before it is applied.

use std::collections::BTreeMap;

use crate::{Ambient, BgShapes, CoverShape, Prefs, RadioExplore, RadioKind, RadioNewShare, RadioUnheard, SourceKey};

/// What the settings call the values of the user's own.
pub const CUSTOM: &str = "Свои";

/// Which defaults of the settings the prefs were written with: 1 brought
/// the presets (the values of the user's own were kept once, [`upgrade`]).
pub const SETTINGS_REV: u8 = 1;

/// A group of preferences with presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum Area {
    /// «Музыка → Рекомендации».
    Recs,
    /// «Тексты → Вид текста»: no presets yet (TODO 36), only its rows.
    Lyrics,
    /// «Вид → Экран трека» (tiles).
    Track,
    /// «Загрузки → Откуда качать».
    Sources,
    /// «Загрузки → Раздача».
    Seeding,
    /// «Система → Нагрузка».
    Load,
}

/// A field of [`Prefs`] a preset owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Field {
    // «Экран трека».
    CoverShape,
    CoverSpin,
    CoverPulse,
    Ring,
    RingBeat,
    SpinRing,
    Ambient,
    BgShapes,
    ProgressWave,
    // «Рекомендации».
    MixesDownloadedOnly,
    RadioNewShare,
    /// The share of new music by the kind of radio: a preset clears it.
    RadioNewShareBy,
    MixesWithNew,
    RadioExplore,
    RadioUnheard,
    // «Откуда качать»: a source's switch (its place stays).
    Source(SourceKey),
    // «Раздача».
    NoSeed,
    SeedLimitGb,
    SeedDays,
    SeedRatio,
    // «Нагрузка».
    FpsLimit,
    TrayKeepWindow,
    AutoDownloadLimit,
    StreamAnalysis,
    LyricsSeparate,
    LyricsDeep,
}

/// The value of a [`Field`].
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FieldValue {
    Bool(bool),
    U32(u32),
    F32(f32),
    Shape(CoverShape),
    Ambient(Ambient),
    BgShapes(BgShapes),
    Share(RadioNewShare),
    ShareBy(BTreeMap<RadioKind, RadioNewShare>),
    Explore(RadioExplore),
    Unheard(RadioUnheard),
}

/// The values of an area's fields kept for «Свои».
pub type Snapshot = Vec<(Field, FieldValue)>;

const TRACK_FIELDS: [Field; 9] = [
    Field::CoverShape,
    Field::CoverSpin,
    Field::CoverPulse,
    Field::Ring,
    Field::RingBeat,
    Field::SpinRing,
    Field::Ambient,
    Field::BgShapes,
    Field::ProgressWave,
];

const RECS_FIELDS: [Field; 6] = [
    Field::MixesDownloadedOnly,
    Field::RadioNewShare,
    Field::RadioNewShareBy,
    Field::MixesWithNew,
    Field::RadioExplore,
    Field::RadioUnheard,
];

const SOURCE_FIELDS: [Field; 8] = [
    Field::Source(SourceKey::Torrents),
    Field::Source(SourceKey::Khinsider),
    Field::Source(SourceKey::Bandcamp),
    Field::Source(SourceKey::Soulseek),
    Field::Source(SourceKey::Qobuz),
    Field::Source(SourceKey::Tidal),
    Field::Source(SourceKey::Squidify),
    Field::Source(SourceKey::Archive),
];

const SEED_FIELDS: [Field; 4] = [Field::NoSeed, Field::SeedLimitGb, Field::SeedDays, Field::SeedRatio];

const LOAD_FIELDS: [Field; 6] = [
    Field::FpsLimit,
    Field::TrayKeepWindow,
    Field::AutoDownloadLimit,
    Field::StreamAnalysis,
    Field::LyricsSeparate,
    Field::LyricsDeep,
];

/// «Экран трека»: shape, spin, pulse, ring, ring in time, ring spin
/// (`None`: «—», the ring is off), glow, background shapes, wave.
type TrackRow = (CoverShape, bool, bool, bool, Option<bool>, Option<bool>, Ambient, BgShapes, bool);
const TRACK: [TrackRow; 3] = [
    (CoverShape::Original, false, false, false, None, None, Ambient::Glow, BgShapes::Off, true),
    (CoverShape::Cookie12, true, true, true, Some(true), Some(true), Ambient::Reactive, BgShapes::Off, true),
    (CoverShape::Random, true, true, true, Some(true), Some(true), Ambient::Reactive, BgShapes::Morph, true),
];

/// «Рекомендации»: only downloaded, the share of new music, mixes with
/// new music, random songs, unheard songs.
type RecsRow = (bool, RadioNewShare, bool, RadioExplore, RadioUnheard);
const RECS: [RecsRow; 3] = [
    (false, RadioNewShare::None, false, RadioExplore::Never, RadioUnheard::Usual),
    (false, RadioNewShare::Some, true, RadioExplore::Rare, RadioUnheard::Usual),
    (false, RadioNewShare::More, true, RadioExplore::Sometimes, RadioUnheard::More),
];

/// «Откуда качать»: which presets switch a source on («Законное»,
/// «Обычно», «Всё»).
const fn source_row(k: SourceKey) -> [bool; 3] {
    match k {
        SourceKey::Bandcamp | SourceKey::Archive => [true, true, true],
        SourceKey::Torrents | SourceKey::Khinsider | SourceKey::Qobuz | SourceKey::Tidal => [false, true, true],
        SourceKey::Soulseek | SourceKey::Squidify => [false, false, true],
    }
}

/// «Раздача»: no seeding, then the limit (GB), the days and the ratio
/// (`None`: «—», nothing is seeded).
type SeedRow = (bool, Option<(u32, u32, f32)>);
const SEED: [SeedRow; 3] = [(true, None), (false, Some((50, 30, 3.5))), (false, Some((200, 90, 0.0)))];

/// «Нагрузка»: frames, quick opening from the tray, downloads at once,
/// stream analysis, voice separation, deep subtitles.
type LoadRow = (u32, bool, u32, bool, bool, bool);
const LOAD: [LoadRow; 3] = [
    (60, false, 1, false, false, false),
    (120, true, 2, true, true, false),
    (142, true, 3, true, true, true),
];

impl Area {
    pub const ALL: [Self; 6] = [Self::Recs, Self::Lyrics, Self::Track, Self::Sources, Self::Seeding, Self::Load];

    /// The presets' names, without «Свои»; empty: the area has none yet.
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Self::Recs => &["Только моё", "Сбалансировано", "Больше нового"],
            Self::Lyrics => &[],
            Self::Track => &["Строго", "Живо", "Максимум"],
            Self::Sources => &["Законное", "Обычно", "Всё"],
            Self::Seeding => &["Не раздавать", "Обычно", "Щедро"],
            Self::Load => &["Тихо", "Обычно", "Быстро"],
        }
    }

    /// The presets are live tiles (the areas with a stage), not buttons.
    pub const fn tiles(self) -> bool {
        matches!(self, Self::Track | Self::Lyrics)
    }

    /// The fields the presets own, for the rows they change.
    pub fn fields(self) -> &'static [Field] {
        match self {
            Self::Recs => &RECS_FIELDS,
            Self::Lyrics => &[],
            Self::Track => &TRACK_FIELDS,
            Self::Sources => &SOURCE_FIELDS,
            Self::Seeding => &SEED_FIELDS,
            Self::Load => &LOAD_FIELDS,
        }
    }

    /// The values preset `n` writes; a field it leaves as it is («—» in
    /// its table) is not there. Empty for a preset there is not.
    pub fn values(self, n: usize) -> Snapshot {
        use FieldValue as V;
        let mut out = Vec::new();
        match self {
            Self::Track => {
                let Some(&(shape, spin, pulse, ring, beat, ring_spin, ambient, bg, wave)) = TRACK.get(n) else { return out };
                out.extend([
                    (Field::CoverShape, V::Shape(shape)),
                    (Field::CoverSpin, V::Bool(spin)),
                    (Field::CoverPulse, V::Bool(pulse)),
                    (Field::Ring, V::Bool(ring)),
                ]);
                out.extend(beat.map(|b| (Field::RingBeat, V::Bool(b))));
                out.extend(ring_spin.map(|b| (Field::SpinRing, V::Bool(b))));
                out.extend([
                    (Field::Ambient, V::Ambient(ambient)),
                    (Field::BgShapes, V::BgShapes(bg)),
                    (Field::ProgressWave, V::Bool(wave)),
                ]);
            }
            Self::Recs => {
                let Some(&(downloaded, share, with_new, explore, unheard)) = RECS.get(n) else { return out };
                out.extend([
                    (Field::MixesDownloadedOnly, V::Bool(downloaded)),
                    (Field::RadioNewShare, V::Share(share)),
                    (Field::RadioNewShareBy, V::ShareBy(BTreeMap::new())),
                    (Field::MixesWithNew, V::Bool(with_new)),
                    (Field::RadioExplore, V::Explore(explore)),
                    (Field::RadioUnheard, V::Unheard(unheard)),
                ]);
            }
            Self::Sources => {
                if n < 3 {
                    out.extend(SOURCE_FIELDS.iter().map(|&f| {
                        let Field::Source(k) = f else { unreachable!("only sources here") };
                        (f, V::Bool(source_row(k)[n]))
                    }));
                }
            }
            Self::Seeding => {
                let Some(&(no_seed, limits)) = SEED.get(n) else { return out };
                out.push((Field::NoSeed, V::Bool(no_seed)));
                if let Some((gb, days, ratio)) = limits {
                    out.extend([
                        (Field::SeedLimitGb, V::U32(gb)),
                        (Field::SeedDays, V::U32(days)),
                        (Field::SeedRatio, V::F32(ratio)),
                    ]);
                }
            }
            Self::Load => {
                let Some(&(fps, tray, downloads, analysis, separate, deep)) = LOAD.get(n) else { return out };
                out.extend([
                    (Field::FpsLimit, V::U32(fps)),
                    (Field::TrayKeepWindow, V::Bool(tray)),
                    (Field::AutoDownloadLimit, V::U32(downloads)),
                    (Field::StreamAnalysis, V::Bool(analysis)),
                    (Field::LyricsSeparate, V::Bool(separate)),
                    (Field::LyricsDeep, V::Bool(deep)),
                ]);
            }
            Self::Lyrics => {}
        }
        out
    }

    /// The preset `p` is in; `None`: «Свои» (also for an area without
    /// presets). A field whose parent is off is not compared.
    pub fn current(self, p: &Prefs) -> Option<usize> {
        (0..self.names().len()).find(|&n| self.values(n).iter().all(|(f, v)| !f.counts(p) || f.matches(p, v)))
    }

    /// `p` with preset `n`, nothing kept: what the settings show before it
    /// is applied.
    pub fn with(self, p: &Prefs, n: usize) -> Prefs {
        let mut out = p.clone();
        for (f, v) in self.values(n) {
            f.set(&mut out, &v);
        }
        out
    }

    /// Preset `n` applied to `p` ([`crate::PrefPatch::Preset`]): from
    /// «Свои», the user's values are kept first for [`Self::custom`].
    pub fn apply(self, p: &mut Prefs, n: usize) {
        if n >= self.names().len() {
            return;
        }
        if self.current(p).is_none() {
            p.custom_presets.insert(self, self.snapshot(p));
        }
        *p = self.with(p, n);
    }

    /// The area's fields as `p` has them.
    pub fn snapshot(self, p: &Prefs) -> Snapshot {
        self.fields().iter().map(|&f| (f, f.get(p))).collect()
    }

    /// `p` with the values kept for «Свои» back; `None`: none were kept.
    pub fn custom(self, p: &Prefs) -> Option<Prefs> {
        let kept = p.custom_presets.get(&self)?;
        let mut out = p.clone();
        for (f, v) in kept {
            f.set(&mut out, v);
        }
        Some(out)
    }

    /// «Свои» is offered: values were kept, or the fields match no preset.
    pub fn custom_shown(self, p: &Prefs) -> bool {
        !self.names().is_empty() && (p.custom_presets.contains_key(&self) || self.current(p).is_none())
    }
}

impl Field {
    /// The area whose presets own the field.
    pub const fn area(self) -> Area {
        match self {
            Self::CoverShape
            | Self::CoverSpin
            | Self::CoverPulse
            | Self::Ring
            | Self::RingBeat
            | Self::SpinRing
            | Self::Ambient
            | Self::BgShapes
            | Self::ProgressWave => Area::Track,
            Self::MixesDownloadedOnly
            | Self::RadioNewShare
            | Self::RadioNewShareBy
            | Self::MixesWithNew
            | Self::RadioExplore
            | Self::RadioUnheard => Area::Recs,
            Self::Source(_) => Area::Sources,
            Self::NoSeed | Self::SeedLimitGb | Self::SeedDays | Self::SeedRatio => Area::Seeding,
            Self::FpsLimit
            | Self::TrayKeepWindow
            | Self::AutoDownloadLimit
            | Self::StreamAnalysis
            | Self::LyricsSeparate
            | Self::LyricsDeep => Area::Load,
        }
    }

    /// The field as `p` has it.
    pub fn get(self, p: &Prefs) -> FieldValue {
        use FieldValue as V;
        match self {
            Self::CoverShape => V::Shape(p.cover_shape),
            Self::CoverSpin => V::Bool(p.cover_spin),
            Self::CoverPulse => V::Bool(p.cover_pulse),
            Self::Ring => V::Bool(p.ring),
            Self::RingBeat => V::Bool(p.ring_beat),
            Self::SpinRing => V::Bool(p.spin_ring),
            Self::Ambient => V::Ambient(p.ambient),
            Self::BgShapes => V::BgShapes(p.bg_shapes),
            Self::ProgressWave => V::Bool(p.progress_wave),
            Self::MixesDownloadedOnly => V::Bool(p.mixes_downloaded_only),
            Self::RadioNewShare => V::Share(p.radio_new_share),
            Self::RadioNewShareBy => V::ShareBy(p.radio_new_share_by.clone()),
            Self::MixesWithNew => V::Bool(p.mixes_with_new),
            Self::RadioExplore => V::Explore(p.radio_explore),
            Self::RadioUnheard => V::Unheard(p.radio_unheard),
            Self::Source(k) => V::Bool(p.source_on(k)),
            Self::NoSeed => V::Bool(p.no_seed),
            Self::SeedLimitGb => V::U32(p.seed_limit_gb),
            Self::SeedDays => V::U32(p.seed_days),
            Self::SeedRatio => V::F32(p.seed_ratio),
            Self::FpsLimit => V::U32(p.fps_limit),
            Self::TrayKeepWindow => V::Bool(p.tray_keep_window),
            Self::AutoDownloadLimit => V::U32(p.auto_download_limit),
            Self::StreamAnalysis => V::Bool(p.stream_analysis),
            Self::LyricsSeparate => V::Bool(p.lyrics_separate),
            Self::LyricsDeep => V::Bool(p.lyrics_deep),
        }
    }

    /// Writes `v` into `p`; a value of another kind changes nothing.
    pub fn set(self, p: &mut Prefs, v: &FieldValue) {
        use FieldValue as V;
        match (self, v) {
            (Self::CoverShape, V::Shape(s)) => p.cover_shape = *s,
            (Self::CoverSpin, V::Bool(b)) => p.cover_spin = *b,
            (Self::CoverPulse, V::Bool(b)) => p.cover_pulse = *b,
            (Self::Ring, V::Bool(b)) => p.ring = *b,
            (Self::RingBeat, V::Bool(b)) => p.ring_beat = *b,
            (Self::SpinRing, V::Bool(b)) => p.spin_ring = *b,
            (Self::Ambient, V::Ambient(a)) => p.ambient = *a,
            (Self::BgShapes, V::BgShapes(b)) => p.bg_shapes = *b,
            (Self::ProgressWave, V::Bool(b)) => p.progress_wave = *b,
            (Self::MixesDownloadedOnly, V::Bool(b)) => p.mixes_downloaded_only = *b,
            (Self::RadioNewShare, V::Share(s)) => p.radio_new_share = *s,
            (Self::RadioNewShareBy, V::ShareBy(m)) => p.radio_new_share_by = m.clone(),
            (Self::MixesWithNew, V::Bool(b)) => p.mixes_with_new = *b,
            (Self::RadioExplore, V::Explore(e)) => p.radio_explore = *e,
            (Self::RadioUnheard, V::Unheard(u)) => p.radio_unheard = *u,
            (Self::Source(k), V::Bool(on)) => {
                if p.source_on(k) != *on {
                    let mut order = p.source_order();
                    if let Some(s) = order.iter_mut().find(|s| s.key == k) {
                        s.on = *on;
                    }
                    p.download_sources = order;
                }
            }
            (Self::NoSeed, V::Bool(b)) => p.no_seed = *b,
            (Self::SeedLimitGb, V::U32(n)) => p.seed_limit_gb = *n,
            (Self::SeedDays, V::U32(n)) => p.seed_days = *n,
            (Self::SeedRatio, V::F32(x)) => p.seed_ratio = *x,
            (Self::FpsLimit, V::U32(n)) => p.fps_limit = *n,
            (Self::TrayKeepWindow, V::Bool(b)) => p.tray_keep_window = *b,
            (Self::AutoDownloadLimit, V::U32(n)) => p.auto_download_limit = *n,
            (Self::StreamAnalysis, V::Bool(b)) => p.stream_analysis = *b,
            (Self::LyricsSeparate, V::Bool(b)) => p.lyrics_separate = *b,
            (Self::LyricsDeep, V::Bool(b)) => p.lyrics_deep = *b,
            _ => {}
        }
    }

    /// `p` has the preset's value `v`. The share by the kind of radio
    /// matches when no kind differs from the share everywhere (a preset
    /// clears it).
    pub fn matches(self, p: &Prefs, v: &FieldValue) -> bool {
        match (self, v) {
            (Self::RadioNewShareBy, _) => p.radio_new_share_by.values().all(|s| *s == p.radio_new_share),
            (_, FieldValue::F32(x)) => matches!(self.get(p), FieldValue::F32(y) if (x - y).abs() < 1e-3),
            _ => self.get(p) == *v,
        }
    }

    /// The field counts in `p`: its parent row is on (the ring for its
    /// beat and spin, seeding for its limits, new music for the random
    /// songs). One that does not count never makes a preset «Свои».
    pub fn counts(self, p: &Prefs) -> bool {
        match self {
            Self::RingBeat | Self::SpinRing => p.ring,
            Self::SeedLimitGb | Self::SeedDays | Self::SeedRatio => !p.no_seed,
            Self::RadioExplore => p.radio_new_share != RadioNewShare::None,
            _ => true,
        }
    }
}

/// Prefs written with the settings of revision `stored` (0: before the
/// presets) take this revision once: an area matching no preset keeps
/// the user's values for «Свои» (nothing else changes), and the settings
/// open in «Подробно» when `deep` (the owner's own build) — a new user's
/// prefs never come here.
pub fn upgrade(p: &mut Prefs, stored: u8, deep: bool) {
    if stored < 1 {
        for area in Area::ALL {
            if !area.names().is_empty() && area.current(p).is_none() && !p.custom_presets.contains_key(&area) {
                p.custom_presets.insert(area, area.snapshot(p));
            }
        }
        p.settings_deep = deep;
    }
    p.settings_rev = SETTINGS_REV;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_presets() -> impl Iterator<Item = Area> {
        Area::ALL.into_iter().filter(|a| !a.names().is_empty())
    }

    #[test]
    fn each_preset_matches_itself_once_applied() {
        for base in [Prefs::default(), Prefs::first_start()] {
            for area in with_presets() {
                for n in 0..area.names().len() {
                    let p = area.with(&base, n);
                    assert_eq!(area.current(&p), Some(n), "{area:?} {n}");
                    let mut q = base.clone();
                    area.apply(&mut q, n);
                    assert_eq!(area.current(&q), Some(n), "{area:?} {n} applied");
                }
            }
        }
    }

    #[test]
    fn presets_of_one_area_differ() {
        for area in with_presets() {
            for a in 0..area.names().len() {
                for b in 0..area.names().len() {
                    let p = area.with(&Prefs::default(), a);
                    assert_eq!(area.current(&area.with(&p, b)) == Some(a), a == b, "{area:?} {a} {b}");
                }
            }
        }
    }

    #[test]
    fn a_first_start_is_in_the_planned_presets() {
        let p = Prefs::first_start();
        assert_eq!(Area::Recs.current(&p), Some(1), "Сбалансировано");
        assert_eq!(Area::Track.current(&p), Some(2), "Максимум");
        assert_eq!(Area::Sources.current(&p), Some(1), "Обычно");
        assert_eq!(Area::Seeding.current(&p), Some(1), "Обычно");
        assert_eq!(Area::Load.current(&p), Some(2), "Быстро");
        for area in Area::ALL {
            assert!(!area.custom_shown(&p), "{area:?}");
        }
    }

    #[test]
    fn the_old_defaults_are_where_the_plan_says() {
        let p = Prefs::default();
        assert_eq!(Area::Track.current(&p), Some(1), "Живо");
        assert_eq!(Area::Sources.current(&p), Some(1), "Обычно");
        assert_eq!(Area::Recs.current(&p), None, "mixes without new music");
        assert_eq!(Area::Seeding.current(&p), None, "ratio 2.0");
        assert_eq!(Area::Load.current(&p), None, "60 frames");
        assert_eq!(Area::Lyrics.current(&p), None);
        assert!(!Area::Lyrics.custom_shown(&p), "no presets, no «Свои»");
    }

    #[test]
    fn fields_whose_parent_is_off_do_not_count() {
        let strict = Area::Track.with(&Prefs::default(), 0);
        assert!(!strict.ring);
        for (beat, spin) in [(false, false), (true, false), (false, true), (true, true)] {
            let p = Prefs { ring_beat: beat, spin_ring: spin, ..strict.clone() };
            assert_eq!(Area::Track.current(&p), Some(0));
        }
        let none = Area::Seeding.with(&Prefs::default(), 0);
        let p = Prefs { seed_limit_gb: 7, seed_days: 1, seed_ratio: 9.0, ..none };
        assert_eq!(Area::Seeding.current(&p), Some(0));
        let mine = Area::Recs.with(&Prefs::default(), 0);
        for explore in RadioExplore::ALL {
            assert_eq!(Area::Recs.current(&Prefs { radio_explore: explore, ..mine.clone() }), Some(0));
        }
        // With the parent on they count.
        let lively = Area::Track.with(&Prefs::default(), 1);
        assert_eq!(Area::Track.current(&Prefs { ring_beat: false, ..lively }), None);
    }

    #[test]
    fn the_rest_of_a_group_never_makes_it_custom() {
        let p = Prefs { cover_ccw: false, ring_style: crate::RingStyle::Bars, bg_shapes_count: 30, ambient_strength: 2.0, ..Prefs::first_start() };
        assert_eq!(Area::Track.current(&p), Some(2));
        let p = Prefs { upload_limit_kbps: 100, download_limit_kbps: 5, ..Prefs::first_start() };
        assert_eq!(Area::Seeding.current(&p), Some(1));
    }

    #[test]
    fn custom_brings_back_every_field() {
        let mut p = Prefs {
            cover_shape: CoverShape::Heart,
            cover_spin: false,
            ring_style: crate::RingStyle::Bars,
            ambient: Ambient::Off,
            no_seed: false,
            seed_limit_gb: 10,
            seed_days: 3,
            seed_ratio: 1.25,
            fps_limit: 90,
            auto_download_limit: 5,
            mixes_downloaded_only: true,
            radio_new_share: RadioNewShare::More,
            radio_explore: RadioExplore::Often,
            ..Prefs::default()
        };
        p.radio_new_share_by.insert(RadioKind::Artist, RadioNewShare::None);
        p.download_sources = vec![crate::SourceSwitch { key: SourceKey::Squidify, on: true }, crate::SourceSwitch { key: SourceKey::Torrents, on: false }];
        let mine = p.clone();
        for area in with_presets() {
            let before: Vec<_> = area.snapshot(&p);
            let custom = area.current(&p).is_none();
            area.apply(&mut p, 0);
            assert_eq!(area.current(&p), Some(0));
            assert!(area.custom_shown(&p) == custom, "{area:?}");
            if custom {
                let back = area.custom(&p).unwrap();
                assert_eq!(area.snapshot(&back), before, "{area:?}");
                p = back;
            }
        }
        // Every owned field is back; the source order too.
        for area in with_presets() {
            assert_eq!(area.snapshot(&p), area.snapshot(&mine), "{area:?}");
        }
        assert_eq!(p.source_order(), mine.source_order());
        assert_eq!(p.radio_new_share_by, mine.radio_new_share_by);
    }

    #[test]
    fn a_preset_over_a_preset_keeps_the_snapshot() {
        let mut p = Prefs { fps_limit: 90, ..Prefs::default() };
        Area::Load.apply(&mut p, 0);
        let kept = p.custom_presets.get(&Area::Load).cloned();
        Area::Load.apply(&mut p, 2);
        assert_eq!(p.custom_presets.get(&Area::Load).cloned(), kept, "from a preset: the snapshot stays");
        assert_eq!(Area::Load.custom(&p).unwrap().fps_limit, 90);
        // A preset from a first start keeps nothing: no «Свои».
        let mut f = Prefs::first_start();
        Area::Load.apply(&mut f, 0);
        assert!(!f.custom_presets.contains_key(&Area::Load) && !Area::Load.custom_shown(&f));
    }

    #[test]
    fn the_recs_preset_clears_the_share_by_kind() {
        let mut p = Prefs::first_start();
        p.radio_new_share_by.insert(RadioKind::Mix, RadioNewShare::None);
        assert_eq!(Area::Recs.current(&p), None);
        // The same share by kind as everywhere: still the preset.
        let same = Prefs { radio_new_share_by: [(RadioKind::Mix, RadioNewShare::Some)].into(), ..Prefs::first_start() };
        assert_eq!(Area::Recs.current(&same), Some(1));
        Area::Recs.apply(&mut p, 2);
        assert!(p.radio_new_share_by.is_empty());
        assert_eq!(Area::Recs.custom(&p).unwrap().radio_new_share_by.get(&RadioKind::Mix), Some(&RadioNewShare::None));
    }

    #[test]
    fn sources_keep_their_order() {
        let order = vec![
            crate::SourceSwitch { key: SourceKey::Archive, on: false },
            crate::SourceSwitch { key: SourceKey::Soulseek, on: true },
        ];
        let p = Prefs { download_sources: order, ..Prefs::default() };
        let keys = |p: &Prefs| p.source_order().iter().map(|s| s.key).collect::<Vec<_>>();
        for n in 0..3 {
            let q = Area::Sources.with(&p, n);
            assert_eq!(keys(&q), keys(&p));
        }
        let lawful = Area::Sources.with(&p, 0);
        assert!(lawful.source_on(SourceKey::Archive) && lawful.source_on(SourceKey::Bandcamp));
        assert!(!lawful.source_on(SourceKey::Torrents) && !lawful.source_on(SourceKey::Soulseek));
    }

    #[test]
    fn no_preset_replaces_or_converts_by_itself() {
        for base in [Prefs::default(), Prefs::first_start()] {
            for area in with_presets() {
                for n in 0..area.names().len() {
                    let p = area.with(&base, n);
                    assert_eq!((p.upgrade_auto, p.fake_flac_auto), (base.upgrade_auto, base.fake_flac_auto));
                }
            }
        }
    }

    #[test]
    fn every_field_belongs_to_its_area() {
        for area in Area::ALL {
            for f in area.fields() {
                assert_eq!(f.area(), area, "{f:?}");
            }
            for n in 0..area.names().len() {
                for (f, _) in area.values(n) {
                    assert!(area.fields().contains(&f), "{area:?} {f:?}");
                }
            }
        }
    }

    #[test]
    fn the_snapshot_is_kept_once() {
        let mut p = Prefs::default();
        upgrade(&mut p, 0, false);
        assert_eq!(p.settings_rev, SETTINGS_REV);
        let kept: Vec<_> = p.custom_presets.keys().copied().collect();
        assert_eq!(kept, vec![Area::Recs, Area::Seeding, Area::Load]);
        // Read again (revision 1 now) after a change: the snapshot stays.
        p.fps_limit = 30;
        let before = p.custom_presets.clone();
        upgrade(&mut p, SETTINGS_REV, true);
        assert_eq!(p.custom_presets, before);
        assert!(!p.settings_deep, "only old prefs take the mode");
        // Nothing but the snapshots changes.
        let mut q = Prefs::default();
        upgrade(&mut q, 0, true);
        assert!(q.settings_deep);
        assert_eq!(Prefs { custom_presets: BTreeMap::new(), settings_deep: false, ..q }, Prefs::default());
    }
}
