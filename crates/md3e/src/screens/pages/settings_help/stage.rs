
use nikgob_core::i18n::Locale;
use nikgob_view::{self as v, Prefs};
use nikgob_core::i18n::settings::{
    fmt_days_in, fmt_gb_in, fmt_one_decimal_in, settings_misc_in,
    settings_row_labels_in,
};

use super::super::home::Section;
use crate::screens::stage::{Part, Scene, Script};

use super::*;


/// What the live stage shows for a row.
impl SettingId {
    /// The screen on the stage while the pointer is on the row; `None`: the
    /// one there already (the theme shows on any).
    pub fn scene(self) -> Option<Scene> {
        use SettingId::*;
        match self {
            NpMotion | Preset(v::presets::Area::Track) | PlayerButton(_) => Some(Scene::Track),
            Preset(v::presets::Area::Lyrics) => Some(Scene::Lyrics),
            HomeSections => Some(Scene::Home),
            LikedAlbumsPlace => Some(Scene::Album),
            _ => self.part().and_then(Part::scene),
        }
    }

    /// The part of the screen the row changes; `None`: the row is not on
    /// the stage. [`Part::Whole`]: all of it, nothing outlined.
    pub fn part(self) -> Option<Part> {
        use SettingId::*;
        Some(match self {
            Theme | Contrast | Scheme | ReducedMotion | NpMotion | HomeSections | PlayerButton(_) => Part::Whole,
            Preset(v::presets::Area::Track | v::presets::Area::Lyrics) => Part::Whole,
            CoverShape | ShapeRange | ShapePick | CoverSpin | CoverDir | CoverSpinSpeed | CoverPulse | CoverPulseStrength
            | CoverPauseMorph | CoverClickPause | DblClickShape | TapWait => Part::Cover,
            Ring | RingStyle => Part::Ring,
            Ambient | AmbientStrength | BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | BgHue => {
                Part::Background
            }
            ProgressWave => Part::Progress,
            NpVolume | NpVolumeHold => Part::Volume,
            LyricsStyle | LyricsUnit | LyricsSmooth | LightSpeed | WaveJump | JumpSpeed | LyricsLift | LyricsLead | LyricsDim
            | LyricsGlow | WaveFallback => Part::LyricsLine,
            LyricsScale | LyricsCascade | LyricsReturn => Part::LyricsColumn,
            LyricsSoftEdges | LyricsSoftDepth | LyricsSoftStrength | LyricsSoftFade => Part::LyricsEdges,
            LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize | LyricsCountdown => Part::LyricsPause,
            MixesProfiles | MixesEverything | MixesMoods | MixesOnRepeat | BackgroundCards | ProfileNames
            | HiddenMixes => Part::Home(Section::Mixes),
            AlbumDate => Part::AlbumDate,
            _ => return None,
        })
    }

    /// The parts outlined for the row: its own, or every part a preset
    /// changes (none for [`Part::Whole`]).
    pub fn parts(self) -> Vec<Part> {
        use v::presets::Area;
        match self {
            Self::Preset(Area::Track) => vec![Part::Cover, Part::Ring, Part::Background, Part::Progress],
            Self::Preset(Area::Lyrics) => vec![Part::LyricsLine, Part::LyricsColumn],
            _ => self.part().filter(|p| *p != Part::Whole).into_iter().collect(),
        }
    }

    /// The settings the stage shows for the row from `p`: where the row
    /// tunes something the shown look does not have (a jump without the
    /// wave, crests on a flat ring, a speed of a line that lights whole),
    /// the look that has it, so every knob shows on the stage. Nothing of it
    /// is kept.
    pub fn staged(self, p: &Prefs) -> Prefs {
        use SettingId::*;
        let mut q = p.clone();
        match self {
            WaveJump | JumpSpeed if q.lyrics_style != 2 => q.lyrics_style = 2,
            LyricsLift if q.lyrics_style != 0 => q.lyrics_style = 0,
            LyricsGapWaves | LyricsGapHeight if q.lyrics_gap_style % 2 == 0 => q.lyrics_gap_style += 1,
            LyricsSmooth | LightSpeed if q.lyrics_style == 1 && q.lyrics_unit == v::LYRICS_BY_LETTERS => {
                q.lyrics_unit = v::LYRICS_BY_SYLLABLES;
            }
            _ => {}
        }
        q
    }

    /// What the stage acts out for the row with the settings `p` shown: what
    /// cannot be seen without doing something.
    pub fn script(self, p: &Prefs) -> Script {
        use SettingId::*;
        match self {
            // A shape of the pool under the pointer is shown as it is.
            CoverShape | ShapeRange if p.cover_shape == v::CoverShape::Random => Script::RandomShape,
            CoverPauseMorph | CoverClickPause => Script::PausePlay,
            DblClickShape | TapWait => Script::DoubleTap,
            NpVolume | NpVolumeHold => Script::Volume,
            NpMotion => Script::Open,
            LyricsCascade => Script::LyricsStep,
            LyricsReturn => Script::LyricsReturn,
            LyricsSoftEdges | LyricsSoftDepth | LyricsSoftStrength | LyricsSoftFade => Script::EdgesScroll,
            LyricsGap | LyricsGapWaves | LyricsGapHeight | LyricsGapSize => Script::Interlude,
            LyricsCountdown => Script::Countdown,
            WaveFallback => Script::LineTimes,
            _ => Script::None,
        }
    }

    /// Whether the stage should pulse on a beat while this row is shown.
    /// Cover shape, ring style and the rest stay calm, so the shape can be
    /// judged. Only a row whose change is the beat itself pulses, and only
    /// while that switch is on: pulse off, or ring beat off, stays calm.
    pub fn needs_beat(self, p: &Prefs) -> bool {
        use SettingId::*;
        match self {
            Ambient | AmbientStrength => p.ambient == v::Ambient::Reactive,
            Ring => p.ring,
            CoverPulse | CoverPulseStrength => p.cover_pulse,
            BgReact => p.bg_shapes != v::BgShapes::Off,
            Preset(v::presets::Area::Track) => {
                p.cover_pulse
                    || p.ring
                    || p.ambient == v::Ambient::Reactive
                    || (p.bg_shapes != v::BgShapes::Off && p.bg_shapes_react > 0.02)
            }
            _ => false,
        }
    }
}

/// The rows that show a field a preset owns.
pub fn preset_rows(f: v::presets::Field) -> Vec<SettingId> {
    use SettingId as S;
    use v::presets::Field as F;
    match f {
        F::CoverShape => vec![S::CoverShape],
        F::CoverSpin => vec![S::CoverSpin],
        F::CoverPulse => vec![S::CoverPulse],
        F::Ring => vec![S::Ring],
        // The ring's old rows: no row shows them now.
        F::RingBeat | F::SpinRing => Vec::new(),
        F::Ambient => vec![S::Ambient],
        F::BgShapes => vec![S::BgShapes],
        F::ProgressWave => vec![S::ProgressWave],
        F::MixesDownloadedOnly => vec![S::MixesDownloadedOnly],
        F::RadioNewShare => vec![S::RadioNewShare],
        F::RadioNewShareBy => [v::RadioKind::Track, v::RadioKind::Artist, v::RadioKind::Mix, v::RadioKind::Autoplay]
            .into_iter()
            .map(S::NewShareBy)
            .collect(),
        F::MixesWithNew => vec![S::MixesWithNew],
        F::RadioExplore => vec![S::RadioExplore],
        F::RadioUnheard => vec![S::RadioUnheard],
        F::Source(k) => vec![S::DownloadSource(k)],
        F::NoSeed => vec![S::NoSeed],
        F::SeedLimitGb => vec![S::SeedLimit],
        F::SeedDays => vec![S::SeedDays],
        F::SeedRatio => vec![S::SeedRatio],
        F::FpsLimit => vec![S::FpsLimit],
        F::TrayKeepWindow => vec![S::TrayKeepWindow],
        F::AutoDownloadLimit => vec![S::AutoDownloads],
        F::StreamAnalysis => vec![S::StreamAnalysis],
        F::LyricsSeparate => vec![S::LyricsSeparate],
        F::LyricsDeep => vec![S::LyricsDeep],
        F::LyricsStyle => vec![S::LyricsStyle],
        F::LyricsUnit => vec![S::LyricsUnit],
        F::LyricsLightSpeed => vec![S::LightSpeed],
        F::LyricsSmooth => vec![S::LyricsSmooth],
        F::LyricsWaveJump => vec![S::WaveJump],
        F::LyricsJumpSpeed => vec![S::JumpSpeed],
        F::LyricsLift => vec![S::LyricsLift],
        F::LyricsDim => vec![S::LyricsDim],
        F::LyricsGlow => vec![S::LyricsGlow],
        F::LyricsCascade => vec![S::LyricsCascade],
        F::LyricsSoftEdges => vec![S::LyricsSoftEdges],
    }
}

/// A field's name in a preset's card (shorter than its row's, and never
/// the same for two fields of one area).
pub(super) fn field_label_in(loc: Locale, f: v::presets::Field) -> &'static str {
    use v::presets::Field as F;
    let l = settings_row_labels_in(loc);
    let m = settings_misc_in(loc);
    match f {
        F::CoverShape => l.cover_shape,
        F::CoverSpin => m.field_cover_spin,
        F::CoverPulse => l.cover_pulse,
        F::Ring => l.ring,
        F::RingBeat => m.field_ring_beat,
        F::SpinRing => m.field_spin_ring,
        F::Ambient => l.ambient,
        F::BgShapes => l.bg_shapes,
        F::ProgressWave => m.field_progress_wave,
        F::MixesDownloadedOnly => m.field_mixes_downloaded,
        F::RadioNewShare => m.field_new,
        F::RadioNewShareBy => m.field_new_by_kind,
        F::MixesWithNew => m.field_mixes_with_new,
        F::RadioExplore => l.radio_explore,
        F::RadioUnheard => m.field_unheard,
        F::Source(k) => k.display_name(),
        F::NoSeed => l.no_seed,
        F::SeedLimitGb => m.field_limit,
        F::SeedDays => m.field_term,
        F::SeedRatio => l.seed_ratio,
        F::FpsLimit => m.field_frames,
        F::TrayKeepWindow => m.field_tray,
        F::AutoDownloadLimit => m.field_auto_downloads,
        F::StreamAnalysis => m.field_stream_analysis,
        F::LyricsSeparate => l.lyrics_separate,
        F::LyricsDeep => m.field_deep_subtitles,
        F::LyricsStyle => l.lyrics_style,
        F::LyricsUnit => m.field_step,
        F::LyricsLightSpeed => l.light_speed,
        F::LyricsSmooth => l.lyrics_smooth,
        F::LyricsWaveJump => l.wave_jump,
        F::LyricsJumpSpeed => l.jump_speed,
        F::LyricsLift => l.lyrics_lift,
        F::LyricsDim => m.field_dim,
        F::LyricsGlow => l.lyrics_glow,
        F::LyricsCascade => m.field_cascade,
        F::LyricsSoftEdges => l.lyrics_soft_edges,
    }
}

/// A value a preset writes, as its card shows it.
pub(super) fn field_text_in(loc: Locale, f: v::presets::Field, value: &v::presets::FieldValue) -> String {
    use v::presets::FieldValue as V;
    // A choice: the label its row shows for it.
    let choice = || {
        let rows = preset_rows(f);
        let id = rows.first().copied()?;
        let mut p = Prefs::default();
        f.set(&mut p, value);
        id.choices_in(loc).get(id.chosen(&p)).map(|c| c.label.to_owned())
    };
    let m = settings_misc_in(loc);
    match value {
        V::Bool(true) => m.value_yes.to_owned(),
        V::Bool(false) => m.value_no.to_owned(),
        V::U32(n) => match f {
            // 0 is no cap (the torrents' policy), not «0 ГБ».
            v::presets::Field::SeedLimitGb if *n == 0 => m.value_no_limit.to_owned(),
            v::presets::Field::SeedLimitGb => fmt_gb_in(loc, *n),
            v::presets::Field::SeedDays => fmt_days_in(loc, *n),
            _ => n.to_string(),
        },
        // The look of the lyrics: shares, in percent.
        V::F32(x)
            if matches!(
                f,
                v::presets::Field::LyricsLightSpeed
                    | v::presets::Field::LyricsSmooth
                    | v::presets::Field::LyricsWaveJump
                    | v::presets::Field::LyricsJumpSpeed
                    | v::presets::Field::LyricsLift
                    | v::presets::Field::LyricsDim
            ) =>
        {
            format!("{} %", (x * 100.0).round())
        }
        V::F32(x) if x.abs() < 1e-3 => m.value_no_cap.to_owned(),
        V::F32(x) => fmt_one_decimal_in(loc, *x),
        V::Shape(v::CoverShape::Original) => m.value_square.to_owned(),
        V::Shape(v::CoverShape::Random) => m.value_random.to_owned(),
        V::Shape(c) => c.label().to_owned(),
        V::ShareBy(_) => "—".to_owned(),
        _ => choice().unwrap_or_else(|| "—".to_owned()),
    }
}
