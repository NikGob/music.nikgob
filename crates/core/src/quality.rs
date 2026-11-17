//! What the spectrum check found about a file, and how the player names a
//! sound without passing a fake off as lossless.
//!
//! The check itself lives in the importer (`nikgob_import::spectrum`); its
//! result is kept per source in the library ([`QualityCheck`]) and read
//! back here into labels: «FLAC из MP3 ~192» for a FLAC made from an MP3,
//! «MP3 320 из поддельного FLAC» for one converted after the check, and the
//! plain words of the track's quality panel. A cut a real master may have
//! too is a doubt, not a finding: the short labels of the lists and of the
//! player say the plain codec, and only the quality panel words the doubt
//! («FLAC, возможно из MP3 320»).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::i18n::Locale;
use crate::model::{Codec, UnixMs, quality_score, score};

/// Bands of the averaged spectrum kept for the quality panel's chart.
pub const SPECTRUM_POINTS: usize = 256;

/// What the spectrum of a file says about its sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Full-band (or at least not cut like a lossy encoder).
    Genuine,
    /// Lossless container, lossy content (for a lossy file: cut like a lossy
    /// encoder, which is what it is).
    Transcoded,
    /// Hi-res container, CD-rate content.
    Upsampled,
    /// Too quiet, too short or too low a sample rate to tell.
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Genuine => "genuine",
            Self::Transcoded => "transcoded",
            Self::Upsampled => "upsampled",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "genuine" => Self::Genuine,
            "transcoded" => Self::Transcoded,
            "upsampled" => Self::Upsampled,
            _ => Self::Unknown,
        }
    }
}

/// The likely origin of a sound, from its verdict and cutoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SoundOrigin {
    /// A real lossless recording.
    Lossless,
    /// Cut like an MP3 of about this bitrate (an AAC cuts alike: the cutoff
    /// alone cannot tell them apart, so «похоже на» in the words).
    Lossy { kbps: u32 },
    /// CD-quality content in a hi-res container.
    Upsampled,
    Unknown,
}

impl SoundOrigin {
    /// Where the encoders' lowpass puts a cut: LAME 128 kbps at 17 kHz, 160
    /// at 17.5, 192 at 18.6, 224..256 and V0 at 19.4..19.7, 320 at 20..20.5.
    pub fn estimate(verdict: Verdict, cutoff_hz: f32) -> Self {
        match verdict {
            Verdict::Genuine => Self::Lossless,
            Verdict::Upsampled => Self::Upsampled,
            Verdict::Unknown => Self::Unknown,
            Verdict::Transcoded => Self::Lossy {
                kbps: match cutoff_hz {
                    c if c < 17_500.0 => 128,
                    c if c < 19_000.0 => 192,
                    c if c < 19_850.0 => 256,
                    _ => 320,
                },
            },
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Self::Lossless => "lossless".into(),
            Self::Lossy { kbps } => format!("lossy:{kbps}"),
            Self::Upsampled => "upsampled".into(),
            Self::Unknown => "unknown".into(),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "lossless" => Self::Lossless,
            "upsampled" => Self::Upsampled,
            _ => s
                .strip_prefix("lossy:")
                .and_then(|k| k.parse().ok())
                .map_or(Self::Unknown, |kbps| Self::Lossy { kbps }),
        }
    }

    pub fn lossy_kbps(&self) -> Option<u32> {
        match self {
            Self::Lossy { kbps } => Some(*kbps),
            _ => None,
        }
    }
}

impl fmt::Display for SoundOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// The stored result of one check of one source.
#[derive(Clone, Debug, PartialEq)]
pub struct QualityCheck {
    pub verdict: Verdict,
    /// Highest frequency with real content, Hz (0 when unknown).
    pub cutoff_hz: f32,
    /// Level drop across the cutoff, dB.
    pub steepness_db: f32,
    pub origin: SoundOrigin,
    /// Rate of the analysed audio: the spectrum spans 0..rate/2.
    pub sample_rate: u32,
    /// Averaged spectrum, dB re a full-scale sine, [`SPECTRUM_POINTS`]
    /// bands evenly spread from 0 to rate/2 (empty when unknown).
    pub spectrum_db: Vec<f32>,
    /// The check was made on the lossless file this source was made from
    /// (an Opus of a FLAC, an MP3 of a fake FLAC), not on the source's file.
    pub made_from: Option<Codec>,
    /// Size and modification time (Unix ms) of the source's file when it was
    /// checked: another file there is checked again.
    pub file_size: u64,
    pub file_mtime: UnixMs,
    pub checked_at: UnixMs,
}

impl QualityCheck {
    pub fn summary(&self) -> CheckSummary {
        let verdict = plausible(self.verdict, self.cutoff_hz);
        CheckSummary {
            verdict,
            origin: if verdict == self.verdict { self.origin } else { SoundOrigin::Unknown },
            made_from: self.made_from.clone(),
            cutoff_hz: summary_cut(self.cutoff_hz),
        }
    }
}

/// The cut as a summary keeps it: whole Hz, truncated as the rows' SQL
/// does (`CAST(cutoff_hz AS INTEGER)`), none when unknown.
fn summary_cut(hz: f32) -> Option<u32> {
    (hz >= 1.0).then_some(hz as u32)
}

/// What a track row needs of a check: the label, without the spectrum.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSummary {
    pub verdict: Verdict,
    pub origin: SoundOrigin,
    pub made_from: Option<Codec>,
    /// Where the band was cut, whole Hz: whether a transcoding is plain or
    /// only possible ([`CheckSummary::sure`]). `None` in a summary encoded
    /// before it was kept.
    #[serde(default)]
    pub cutoff_hz: Option<u32>,
}

impl Serialize for SoundOrigin {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for SoundOrigin {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(d)?))
    }
}

/// The lowest lowpass a lossy encoder puts on music people share (MP3 at
/// 64 kbps cuts near 11 kHz): content that stops below it is the music's
/// own (a quiet synth, a piano, a chiptune), not a codec's cut.
pub const LOWEST_LOWPASS_HZ: f32 = 11_000.0;

/// A verdict as it stands: a transcoding found with its cut where no lossy
/// encoder puts one says nothing (checks stored before this rule read as
/// [`Verdict::Unknown`]).
pub fn plausible(verdict: Verdict, cutoff_hz: f32) -> Verdict {
    if verdict == Verdict::Transcoded && cutoff_hz < LOWEST_LOWPASS_HZ {
        Verdict::Unknown
    } else {
        verdict
    }
}

/// A check says something about a file only when it is of lossless audio:
/// the file's own when it is lossless, or the lossless original it was made
/// from. A lossy file's own lowpass is what its format does.
pub fn check_counts(codec: &Codec, made_from: Option<&Codec>) -> bool {
    match made_from {
        Some(m) => m.is_lossless(),
        None => codec.is_lossless(),
    }
}

/// Where a cut stops telling an MP3 from a real master: MP3 320 cuts near
/// 20 kHz, and so do a mastering lowpass and the filter of a 48 → 44.1 kHz
/// resampling in genuine lossless files.
pub const HIGHEST_CLEAR_CUT_HZ: f32 = 19_800.0;

/// A fake so plain that turning it into an MP3 loses nothing for sure: a
/// lossy encoder's cliff (a steep drop where MP3s of 128–256 kbps cut,
/// 15 to 19.8 kHz), and a steeper one still for a file that came as hi-res.
/// A cut at 20 kHz and above may be a real master's own filter. Anything
/// less is labelled, never converted.
pub fn clear_fake(verdict: Verdict, cutoff_hz: f32, steepness_db: f32, hi_res: bool) -> bool {
    plausible(verdict, cutoff_hz) == Verdict::Transcoded
        && (15_000.0..HIGHEST_CLEAR_CUT_HZ).contains(&cutoff_hz)
        && steepness_db >= if hi_res { 40.0 } else { 30.0 }
}

/// A transcoding found with this cut is only a possibility: at
/// [`HIGHEST_CLEAR_CUT_HZ`] and above an MP3 320 cuts where a real master's
/// own lowpass or a resampling filter does too.
pub fn unsure_cut(cutoff_hz: f32) -> bool {
    cutoff_hz >= HIGHEST_CLEAR_CUT_HZ
}

impl CheckSummary {
    /// `verdict|origin|made_from|cutoff_hz`, as the library's track rows
    /// select it (the cut in whole Hz, empty when unknown).
    pub fn encode(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.verdict.as_str(),
            self.origin.as_str(),
            self.made_from.as_ref().map_or("", Codec::as_str),
            self.cutoff_hz.map_or_else(String::new, |c| c.to_string())
        )
    }

    /// Reads [`Self::encode`]; a summary of three fields (encoded before
    /// the cut was kept) has no cut.
    pub fn decode(s: &str) -> Option<Self> {
        let mut it = s.split('|');
        let verdict = Verdict::parse(it.next()?);
        let origin = SoundOrigin::parse(it.next()?);
        let made_from = it.next().filter(|m| !m.is_empty()).map(Codec::parse);
        let cutoff_hz = it.next().and_then(|c| c.trim().parse::<f64>().ok()).and_then(|c| summary_cut(c as f32));
        Some(Self { verdict, origin, made_from, cutoff_hz })
    }

    /// The transcoding is plain: cut below [`HIGHEST_CLEAR_CUT_HZ`]. At or
    /// above it the file may be a real master. Without the cut (a summary
    /// from before it was kept) an MP3 320's origin is taken as unsure and
    /// a lower one as plain: those come from cuts well under 19.8 kHz.
    pub fn sure(&self) -> bool {
        match self.cutoff_hz {
            Some(c) => !unsure_cut(c as f32),
            None => self.origin.lossy_kbps() != Some(320),
        }
    }

    /// The source's own codec is lossless and its content is not.
    fn fake(&self, codec: &Codec) -> bool {
        codec.is_lossless() && self.made_from.is_none() && self.verdict == Verdict::Transcoded
    }

    /// A lossy file converted from a lossless one that turned out fake.
    fn from_fake(&self, codec: &Codec) -> bool {
        !codec.is_lossless()
            && self.made_from.as_ref().is_some_and(Codec::is_lossless)
            && self.verdict == Verdict::Transcoded
    }

}

/// `quality_score` of a lossless file found transcoded with its band cut at
/// `cutoff_hz`: what its content really is (an MP3 of the estimated
/// bitrate), and never above [`score::FAKE_LOSSLESS`], below MP3 V0. A cut
/// a real master may have too ([`unsure_cut`]) ranks as an MP3 320, never
/// below it: the file may be what it says.
pub fn fake_score(origin: SoundOrigin, cutoff_hz: f32) -> i32 {
    if unsure_cut(cutoff_hz) {
        return score::MP3_320;
    }
    match origin.lossy_kbps() {
        Some(kbps) => quality_score(&Codec::Mp3, Some(kbps), None, None, false).min(score::FAKE_LOSSLESS),
        None => score::FAKE_LOSSLESS,
    }
}

/// How a fake lossless file is converted: MP3 320 CBR keeps everything an
/// MP3 320 or a 256 had; a lower origin fits in V0 with room to spare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mp3Target {
    Cbr320,
    V0,
}

impl Mp3Target {
    pub fn for_origin(origin: SoundOrigin) -> Self {
        match origin.lossy_kbps() {
            Some(k) if k < 256 => Self::V0,
            _ => Self::Cbr320,
        }
    }
}

/// "FLAC", "Opus", "MP3"…
pub fn codec_name(c: &Codec) -> String {
    match c {
        Codec::Flac => "FLAC".into(),
        Codec::Opus => "Opus".into(),
        Codec::Mp3 => "MP3".into(),
        Codec::Aac => "AAC".into(),
        Codec::Vorbis => "Vorbis".into(),
        Codec::Alac => "ALAC".into(),
        Codec::Wav => "WAV".into(),
        Codec::Other(s) => s.to_uppercase(),
    }
}

/// The codec as a list row and the player's line show it, honest about
/// what the content is when the check is sure of it: «FLAC из MP3 ~192»,
/// «MP3 из поддельного FLAC», else the codec («FLAC», «Opus»). A doubt (a
/// cut a real master may have too, [`CheckSummary::sure`]) is not said
/// here: it belongs to the quality panel ([`label`], [`verdict_text`]).
pub fn row_label_in(codec: &Codec, check: Option<&CheckSummary>, loc: Locale) -> String {
    label_in(codec, None, check.filter(|c| c.sure()), loc)
}

/// [`row_label_in`] in Russian (preserved for callers not yet localized).
pub fn row_label(codec: &Codec, check: Option<&CheckSummary>) -> String {
    row_label_in(codec, check, Locale::Ru)
}

/// The check found the file (or the lossless one it was made from) made
/// from a lossy one, and plainly so: what a warning is shown for. A doubt
/// is worded as one and is no warning.
pub fn plain_fake(codec: &Codec, check: &CheckSummary) -> bool {
    check_counts(codec, check.made_from.as_ref()) && (check.fake(codec) || check.from_fake(codec)) && check.sure()
}

/// The file itself is lossless with a lossy file inside, and the check is
/// sure of it (cut below [`HIGHEST_CLEAR_CUT_HZ`]): the one a user may
/// turn into an MP3 by hand. A doubt, or a lossy file made from a fake,
/// is not.
pub fn mp3_inside(codec: &Codec, check: &CheckSummary) -> bool {
    check.fake(codec) && plain_fake(codec, check)
}

/// A library file there is nothing better than: an Opus (any lossy copy)
/// the import made from a lossless original it did not find fake (ranked
/// as that original, [`score::CD_LOSSLESS`] and up), or a lossless file
/// whose check found it genuine. `rank` is the file's `quality_score`,
/// `check` its check (or the check of the original it was made from). A
/// lossless file not checked yet may still be a fake: not this.
pub fn nothing_better(codec: &Codec, rank: i32, check: Option<&CheckSummary>) -> bool {
    if rank < score::CD_LOSSLESS {
        return false;
    }
    if !codec.is_lossless() {
        // Only the import's copy of a lossless original ranks this high.
        return check.is_none_or(|c| c.verdict != Verdict::Transcoded);
    }
    check.is_some_and(|c| c.made_from.is_none() && c.verdict == Verdict::Genuine)
}

/// The one rule of «really better»: a copy of quality `offer` (a
/// `quality_score`) is better than the library's file (`codec`, `rank`,
/// `check`) only when the file is not one there is nothing better than
/// ([`nothing_better`]) and the copy ranks above it. An MP3 gets a
/// genuine lossless copy; a fake FLAC too; our Opus of a genuine FLAC and
/// a genuine FLAC get nothing.
pub fn really_better(codec: &Codec, rank: i32, check: Option<&CheckSummary>, offer: i32) -> bool {
    !nothing_better(codec, rank, check) && offer > rank
}

/// A genuine CD-quality lossless copy would really be better than the
/// file ([`really_better`]): what «Найти качество лучше» of an album
/// looks for. A file ranked as lossless already is never one.
pub fn can_get_better(codec: &Codec, rank: i32, check: Option<&CheckSummary>) -> bool {
    really_better(codec, rank, check, score::CD_LOSSLESS)
}

/// A row's label ([`row_label`]) says what [`mp3_inside`] does: «FLAC из
/// MP3 ~192», «FLAC из сжатого», or their English equivalents. For a list
/// that has only the label (callers with [`CheckSummary`] should use [`mp3_inside`]).
pub fn row_label_mp3_inside(label: &str) -> bool {
    [Codec::Flac, Codec::Alac, Codec::Wav].iter().any(|c| {
        label
            .strip_prefix(codec_name(c).as_str())
            .is_some_and(|rest| rest.starts_with(" из ") || rest.starts_with(" from "))
    })
}

/// The full label of the quality panel and of the import's questions, with
/// the bitrate where it says something and a doubt worded as one: «MP3 320
/// из поддельного FLAC», «MP3 320 из FLAC, возможно поддельного», «FLAC из
/// MP3 ~192», «FLAC, возможно из MP3 320», «Opus 160» (and English equivalents).
pub fn label_in(codec: &Codec, bitrate: Option<u32>, check: Option<&CheckSummary>, loc: Locale) -> String {
    let name = codec_name(codec);
    let with_rate = match bitrate.filter(|_| !codec.is_lossless()) {
        Some(b) => format!("{name} {b}"),
        None => name.clone(),
    };
    // A lossy file's own check says nothing (see [`check_counts`]).
    let Some(c) = check.filter(|c| check_counts(codec, c.made_from.as_ref())) else { return with_rate };
    if c.fake(codec) {
        return match loc {
            Locale::Ru => match (c.origin.lossy_kbps(), c.sure()) {
                (Some(k), true) => format!("{name} из MP3 ~{k}"),
                (Some(k), false) => format!("{name}, возможно из MP3 {k}"),
                (None, true) => format!("{name} из сжатого"),
                (None, false) => format!("{name}, возможно из сжатого"),
            },
            Locale::En => match (c.origin.lossy_kbps(), c.sure()) {
                (Some(k), true) => format!("{name} from MP3 ~{k}"),
                (Some(k), false) => format!("{name}, possibly from MP3 {k}"),
                (None, true) => format!("{name} from lossy"),
                (None, false) => format!("{name}, possibly from lossy"),
            },
        };
    }
    if c.from_fake(codec) {
        let from = c.made_from.as_ref().map(codec_name).unwrap_or_default();
        return match loc {
            Locale::Ru => {
                if c.sure() {
                    format!("{with_rate} из поддельного {from}")
                } else {
                    format!("{with_rate} из {from}, возможно поддельного")
                }
            }
            Locale::En => {
                if c.sure() {
                    format!("{with_rate} from fake {from}")
                } else {
                    format!("{with_rate} from possibly fake {from}")
                }
            }
        };
    }
    with_rate
}

/// [`label_in`] in Russian (preserved for callers not yet localized).
pub fn label(codec: &Codec, bitrate: Option<u32>, check: Option<&CheckSummary>) -> String {
    label_in(codec, bitrate, check, Locale::Ru)
}

/// Localized frequency label (`"16,1 кГц"` / `"16.1 kHz"`).
pub fn khz_in(hz: f32, loc: Locale) -> String {
    match loc {
        Locale::Ru => format!("{:.1} кГц", hz / 1000.0).replace('.', ","),
        Locale::En => format!("{:.1} kHz", hz / 1000.0),
    }
}

/// "16,1 кГц".
pub fn khz(hz: f32) -> String {
    khz_in(hz, Locale::Ru)
}

/// The verdict in plain words for the quality panel in `loc`. `codec` and
/// `bitrate` are the source's own; the check is one that counts
/// ([`check_counts`]).
pub fn verdict_text_in(codec: &Codec, bitrate: Option<u32>, check: &QualityCheck, loc: Locale) -> String {
    let _ = bitrate;
    let s = check.summary();
    let like = |o: SoundOrigin| match loc {
        Locale::Ru => match o.lossy_kbps() {
            Some(k) => format!("похоже на MP3 ~{k} кбит/с"),
            None => "сжатый источник".to_owned(),
        },
        Locale::En => match o.lossy_kbps() {
            Some(k) => format!("looks like MP3 ~{k} kbps"),
            None => "lossy source".to_owned(),
        },
    };
    // A cut a real master may have too is said as a possibility.
    let maybe = |o: SoundOrigin| match loc {
        Locale::Ru => {
            let from = o.lossy_kbps().map_or_else(|| "сжатого файла".to_owned(), |k| format!("MP3 ~{k} кбит/с"));
            format!("сделан из {from}; срез {} бывает и у настоящих записей", khz_in(check.cutoff_hz, loc))
        }
        Locale::En => {
            let from = o.lossy_kbps().map_or_else(|| "a lossy file".to_owned(), |k| format!("MP3 ~{k} kbps"));
            format!("transcoded from {from}; a {} cutoff also occurs in genuine masters", khz_in(check.cutoff_hz, loc))
        }
    };
    if s.fake(codec) {
        if !s.sure() {
            return match loc {
                Locale::Ru => format!("Возможно, {}", maybe(check.origin)),
                Locale::En => format!("Possibly {}", maybe(check.origin)),
            };
        }
        return match loc {
            Locale::Ru => format!("Не настоящий lossless: сделан из сжатого файла, {}", like(check.origin)),
            Locale::En => format!("Not true lossless: transcoded from a lossy file, {}", like(check.origin)),
        };
    }
    if s.from_fake(codec) {
        let from = check.made_from.as_ref().map(codec_name).unwrap_or_default();
        if !s.sure() {
            return match loc {
                Locale::Ru => format!("Сделан из {from}, который, возможно, {}", maybe(check.origin)),
                Locale::En => format!("Encoded from {from}, possibly {}", maybe(check.origin)),
            };
        }
        return match loc {
            Locale::Ru => format!("Сделан из поддельного {from}: внутри был сжатый звук, {}", like(check.origin)),
            Locale::En => format!("Encoded from fake {from}: contained lossy audio, {}", like(check.origin)),
        };
    }
    match (loc, s.verdict, &check.made_from) {
        (Locale::Ru, Verdict::Unknown, _) => "Не определить: запись слишком тихая или короткая".into(),
        (Locale::En, Verdict::Unknown, _) => "Inconclusive: recording is too quiet or too short".into(),
        (Locale::Ru, Verdict::Upsampled, _) => "Lossless, но не Hi-Res: внутри качество CD, растянутое до большей частоты".into(),
        (Locale::En, Verdict::Upsampled, _) => "Lossless, not Hi-Res: CD-quality audio upsampled to a higher rate".into(),
        (Locale::Ru, Verdict::Genuine, Some(from)) => format!("Сделан из настоящего {}", codec_name(from)),
        (Locale::En, Verdict::Genuine, Some(from)) => format!("Encoded from genuine {}", codec_name(from)),
        (Locale::Ru, Verdict::Genuine, None) if codec.is_lossless() => "Настоящий lossless: полоса без среза".into(),
        (Locale::En, Verdict::Genuine, None) if codec.is_lossless() => "True lossless: full frequency band".into(),
        (Locale::Ru, _, _) => "Полоса соответствует формату".into(),
        (Locale::En, _, _) => "Spectrum matches format".into(),
    }
}

/// [`verdict_text_in`] in Russian (preserved for callers not yet localized).
pub fn verdict_text(codec: &Codec, bitrate: Option<u32>, check: &QualityCheck) -> String {
    verdict_text_in(codec, bitrate, check, Locale::Ru)
}

/// «до 16,1 кГц» / «up to 16.1 kHz»; for a full band, «до 22,0 кГц, без среза» / «up to 22.0 kHz, no cutoff».
pub fn cutoff_text_in(check: &QualityCheck, loc: Locale) -> Option<String> {
    if check.verdict == Verdict::Unknown || check.cutoff_hz <= 0.0 {
        return None;
    }
    let nyquist = check.sample_rate as f32 / 2.0;
    let full = check.cutoff_hz >= nyquist * 0.98;
    Some(match loc {
        Locale::Ru => {
            if full {
                format!("до {}, без среза", khz_in(nyquist, loc))
            } else {
                format!("до {}", khz_in(check.cutoff_hz, loc))
            }
        }
        Locale::En => {
            if full {
                format!("up to {}, no cutoff", khz_in(nyquist, loc))
            } else {
                format!("up to {}", khz_in(check.cutoff_hz, loc))
            }
        }
    })
}

/// [`cutoff_text_in`] in Russian (preserved for callers not yet localized).
pub fn cutoff_text(check: &QualityCheck) -> Option<String> {
    cutoff_text_in(check, Locale::Ru)
}

/// Averages `levels_db` (dB, bin `k` of `0..=n-1` spanning 0..nyquist) into
/// `points` bands in the power domain.
pub fn downsample_db(levels_db: &[f32], points: usize) -> Vec<f32> {
    if levels_db.is_empty() || points == 0 {
        return Vec::new();
    }
    let n = levels_db.len();
    (0..points)
        .map(|i| {
            let lo = i * n / points;
            let hi = ((i + 1) * n / points).max(lo + 1).min(n);
            let p: f64 = levels_db[lo..hi].iter().map(|l| 10f64.powf(f64::from(*l) / 10.0)).sum::<f64>()
                / (hi - lo) as f64;
            (10.0 * (p + 1e-20).log10()) as f32
        })
        .collect()
}

/// The spectrum as stored: dB × 10 as little-endian `i16`.
pub fn spectrum_to_blob(db: &[f32]) -> Vec<u8> {
    db.iter()
        .flat_map(|d| ((d * 10.0).round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16).to_le_bytes())
        .collect()
}

pub fn spectrum_from_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 10.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(verdict: Verdict, cutoff: f32, made_from: Option<Codec>) -> QualityCheck {
        QualityCheck {
            verdict,
            cutoff_hz: cutoff,
            steepness_db: 40.0,
            origin: SoundOrigin::estimate(verdict, cutoff),
            sample_rate: 44_100,
            spectrum_db: Vec::new(),
            made_from,
            file_size: 1,
            file_mtime: 1,
            checked_at: 1,
        }
    }

    #[test]
    fn origins_follow_the_encoders_lowpass_and_round_trip() {
        let o = |c| SoundOrigin::estimate(Verdict::Transcoded, c);
        assert_eq!(o(16_000.0), SoundOrigin::Lossy { kbps: 128 });
        assert_eq!(o(17_000.0), SoundOrigin::Lossy { kbps: 128 });
        assert_eq!(o(18_600.0), SoundOrigin::Lossy { kbps: 192 });
        assert_eq!(o(19_600.0), SoundOrigin::Lossy { kbps: 256 });
        assert_eq!(o(20_100.0), SoundOrigin::Lossy { kbps: 320 });
        assert_eq!(SoundOrigin::estimate(Verdict::Genuine, 22_000.0), SoundOrigin::Lossless);
        assert_eq!(SoundOrigin::estimate(Verdict::Upsampled, 22_000.0), SoundOrigin::Upsampled);
        for x in [SoundOrigin::Lossless, SoundOrigin::Lossy { kbps: 192 }, SoundOrigin::Upsampled, SoundOrigin::Unknown] {
            assert_eq!(SoundOrigin::parse(&x.as_str()), x);
        }
        assert_eq!(SoundOrigin::parse("lossy:abc"), SoundOrigin::Unknown);
        for v in [Verdict::Genuine, Verdict::Transcoded, Verdict::Upsampled, Verdict::Unknown] {
            assert_eq!(Verdict::parse(v.as_str()), v);
        }
    }

    #[test]
    fn summaries_round_trip_through_the_row_column() {
        let s = check(Verdict::Transcoded, 16_000.0, Some(Codec::Flac)).summary();
        assert_eq!(s.encode(), "transcoded|lossy:128|flac|16000");
        assert_eq!(CheckSummary::decode(&s.encode()), Some(s));
        let plain = check(Verdict::Genuine, 22_050.0, None).summary();
        assert_eq!(CheckSummary::decode(&plain.encode()), Some(plain));
        assert_eq!(CheckSummary::decode(""), None);
        // The cut is kept in whole Hz, truncated as the rows' SQL does.
        assert_eq!(check(Verdict::Transcoded, 19_799.9, None).summary().cutoff_hz, Some(19_799));
        assert_eq!(check(Verdict::Unknown, 0.0, None).summary().cutoff_hz, None);
        assert_eq!(check(Verdict::Unknown, 0.0, None).summary().encode(), "unknown|unknown||");
        // The SQL gives the cut as an integer; a REAL would read too.
        assert_eq!(CheckSummary::decode("transcoded|lossy:320||20186").unwrap().cutoff_hz, Some(20_186));
        assert_eq!(CheckSummary::decode("transcoded|lossy:320||20186.0").unwrap().cutoff_hz, Some(20_186));
    }

    /// Summaries encoded before the cut was kept (three fields) still read:
    /// an MP3 320's origin is taken as unsure, a lower one as plain.
    #[test]
    fn old_summaries_without_the_cut_still_decode() {
        let old = CheckSummary::decode("transcoded|lossy:320|").unwrap();
        assert_eq!((old.verdict, old.origin, old.made_from.clone(), old.cutoff_hz), (Verdict::Transcoded, SoundOrigin::Lossy { kbps: 320 }, None, None));
        assert!(!old.sure());
        assert_eq!(label(&Codec::Flac, None, Some(&old)), "FLAC, возможно из MP3 320");
        assert_eq!(row_label(&Codec::Flac, Some(&old)), "FLAC");
        let low = CheckSummary::decode("transcoded|lossy:128|").unwrap();
        assert!(low.sure());
        assert_eq!(row_label(&Codec::Flac, Some(&low)), "FLAC из MP3 ~128");
        let conv = CheckSummary::decode("transcoded|lossy:192|flac").unwrap();
        assert_eq!(conv.made_from, Some(Codec::Flac));
        assert_eq!(label(&Codec::Mp3, Some(256), Some(&conv)), "MP3 256 из поддельного FLAC");
        // A JSON summary from before the field reads with no cut.
        let json: CheckSummary = serde_json::from_str(r#"{"verdict":"transcoded","origin":"lossy:320","made_from":null}"#).unwrap();
        assert_eq!(json.cutoff_hz, None);
    }

    /// A cut at 19.8 kHz and above may be a real master's own lowpass or a
    /// resampling filter: the quality panel says it as a possibility, the
    /// rows and the player's line say the plain codec; a cut below it is
    /// said plainly everywhere.
    #[test]
    fn a_cut_a_master_may_have_is_said_as_a_possibility() {
        let maybe = check(Verdict::Transcoded, 20_186.0, None);
        assert_eq!(row_label(&Codec::Flac, Some(&maybe.summary())), "FLAC");
        assert!(!plain_fake(&Codec::Flac, &maybe.summary()));
        assert_eq!(label(&Codec::Flac, Some(1000), Some(&maybe.summary())), "FLAC, возможно из MP3 320");
        assert_eq!(
            verdict_text(&Codec::Flac, None, &maybe),
            "Возможно, сделан из MP3 ~320 кбит/с; срез 20,2 кГц бывает и у настоящих записей"
        );
        // Right at the line: 19.8 kHz is unsure, a hair under it is plain.
        let edge = check(Verdict::Transcoded, 19_800.0, None).summary();
        assert_eq!(label(&Codec::Flac, None, Some(&edge)), "FLAC, возможно из MP3 256");
        assert_eq!(row_label(&Codec::Flac, Some(&edge)), "FLAC");
        let under = check(Verdict::Transcoded, 19_790.0, None).summary();
        assert_eq!(row_label(&Codec::Flac, Some(&under)), "FLAC из MP3 ~256");
        // Converted before plain fakes were told apart (OMORI at 20.1 kHz).
        let conv = check(Verdict::Transcoded, 20_100.0, Some(Codec::Flac));
        assert_eq!(label(&Codec::Mp3, Some(320), Some(&conv.summary())), "MP3 320 из FLAC, возможно поддельного");
        assert_eq!(label(&Codec::Mp3, None, Some(&conv.summary())), "MP3 из FLAC, возможно поддельного");
        assert_eq!(row_label(&Codec::Mp3, Some(&conv.summary())), "MP3");
        assert!(!plain_fake(&Codec::Mp3, &conv.summary()));
        assert_eq!(
            verdict_text(&Codec::Mp3, Some(320), &conv),
            "Сделан из FLAC, который, возможно, сделан из MP3 ~320 кбит/с; срез 20,1 кГц бывает и у настоящих записей"
        );
        // A plain one is said plainly.
        let plain = check(Verdict::Transcoded, 16_100.0, None);
        assert_eq!(row_label(&Codec::Flac, Some(&plain.summary())), "FLAC из MP3 ~128");
        assert!(verdict_text(&Codec::Flac, None, &plain).starts_with("Не настоящий lossless: сделан из сжатого файла"));
        assert!(plain.summary().sure() && !maybe.summary().sure());
        assert!(plain_fake(&Codec::Flac, &plain.summary()));
        let made = check(Verdict::Transcoded, 16_100.0, Some(Codec::Flac)).summary();
        assert!(plain_fake(&Codec::Mp3, &made));
        // A real file is no fake, and neither is a lossy file's own lowpass.
        assert!(!plain_fake(&Codec::Flac, &check(Verdict::Genuine, 22_050.0, None).summary()));
        assert!(!plain_fake(&Codec::Mp3, &plain.summary()));
        // A real file and an unknown one are never «возможно».
        assert_eq!(row_label(&Codec::Flac, Some(&check(Verdict::Genuine, 22_050.0, None).summary())), "FLAC");
        assert_eq!(row_label(&Codec::Flac, Some(&check(Verdict::Upsampled, 21_000.0, None).summary())), "FLAC");
    }

    /// Only a lossless file the check is sure holds an MP3 may be turned
    /// into one by hand; its row label says so too.
    #[test]
    fn only_a_sure_fake_has_an_mp3_inside() {
        let plain = check(Verdict::Transcoded, 16_100.0, None).summary();
        assert!(mp3_inside(&Codec::Flac, &plain));
        assert!(row_label_mp3_inside(&row_label(&Codec::Flac, Some(&plain))));
        let maybe = check(Verdict::Transcoded, 20_186.0, None).summary();
        assert!(!mp3_inside(&Codec::Flac, &maybe), "a doubt");
        assert!(!row_label_mp3_inside(&row_label(&Codec::Flac, Some(&maybe))));
        let made = check(Verdict::Transcoded, 16_100.0, Some(Codec::Flac)).summary();
        assert!(!mp3_inside(&Codec::Mp3, &made), "an MP3 already");
        assert!(!row_label_mp3_inside(&row_label(&Codec::Mp3, Some(&made))));
        assert!(!row_label_mp3_inside(&label(&Codec::Mp3, Some(320), Some(&made))));
        let real = check(Verdict::Genuine, 22_050.0, None).summary();
        assert!(!mp3_inside(&Codec::Flac, &real));
        assert!(!row_label_mp3_inside(&row_label(&Codec::Flac, Some(&real))));
        assert!(!mp3_inside(&Codec::Mp3, &plain), "a lossy file's own lowpass");
        assert!(row_label_mp3_inside(&row_label(&Codec::Wav, Some(&plain))));
        assert!(!row_label_mp3_inside("FLACиз"));
    }

    #[test]
    fn a_fake_is_never_labelled_lossless() {
        let fake = check(Verdict::Transcoded, 18_600.0, None).summary();
        assert_eq!(row_label(&Codec::Flac, Some(&fake)), "FLAC из MP3 ~192");
        let low = check(Verdict::Transcoded, 16_000.0, None).summary();
        assert_eq!(label(&Codec::Flac, Some(1000), Some(&low)), "FLAC из MP3 ~128");
        // Converted after the check.
        let conv = check(Verdict::Transcoded, 19_600.0, Some(Codec::Flac)).summary();
        assert_eq!(label(&Codec::Mp3, Some(320), Some(&conv)), "MP3 320 из поддельного FLAC");
        assert_eq!(row_label(&Codec::Mp3, Some(&conv)), "MP3 из поддельного FLAC");
        // A real one, and a file not checked yet, keep their codec.
        let real = check(Verdict::Genuine, 22_050.0, None).summary();
        assert_eq!(row_label(&Codec::Flac, Some(&real)), "FLAC");
        assert_eq!(row_label(&Codec::Flac, None), "FLAC");
        let opus = check(Verdict::Genuine, 22_050.0, Some(Codec::Flac)).summary();
        assert_eq!(label(&Codec::Opus, Some(160), Some(&opus)), "Opus 160");
        // A lossy file's own check is not a verdict: its lowpass is its
        // format's.
        let blown = check(Verdict::Transcoded, 16_000.0, None).summary();
        assert_eq!(label(&Codec::Mp3, Some(320), Some(&blown)), "MP3 320");
        let fine = check(Verdict::Genuine, 22_000.0, None).summary();
        assert_eq!(label(&Codec::Mp3, Some(320), Some(&fine)), "MP3 320");
        assert_eq!(label(&Codec::Opus, Some(160), Some(&blown)), "Opus 160");
        assert_eq!(label(&Codec::Aac, Some(256), Some(&fine)), "AAC 256");
        assert!(!check_counts(&Codec::Mp3, None) && check_counts(&Codec::Mp3, Some(&Codec::Flac)));
        assert!(check_counts(&Codec::Flac, None) && !check_counts(&Codec::Opus, Some(&Codec::Mp3)));
    }

    /// Content that ends where no encoder cuts (an OST's quiet synth at
    /// 3 kHz, a piano at 6 kHz) is no fake; a fake is converted only for a
    /// plain cliff.
    #[test]
    fn a_cut_no_encoder_makes_is_no_verdict_and_only_a_cliff_is_clear() {
        assert_eq!(plausible(Verdict::Transcoded, 2_820.0), Verdict::Unknown);
        assert_eq!(plausible(Verdict::Transcoded, 6_000.0), Verdict::Unknown);
        assert_eq!(plausible(Verdict::Transcoded, 16_000.0), Verdict::Transcoded);
        let quiet = check(Verdict::Transcoded, 2_820.0, None);
        assert_eq!(quiet.summary().verdict, Verdict::Unknown);
        assert_eq!(row_label(&Codec::Flac, Some(&quiet.summary())), "FLAC");
        // OMORI's «Title» (2.8 kHz, 31 dB), «WHITE SPACE» (12.4 kHz, 56 dB),
        // a Серега Пират single (15.2 kHz, 19 dB), «Молли» (19.9 kHz, 40 dB).
        assert!(!clear_fake(Verdict::Transcoded, 2_820.0, 31.0, false));
        assert!(!clear_fake(Verdict::Transcoded, 12_380.0, 56.0, false));
        assert!(!clear_fake(Verdict::Transcoded, 15_200.0, 19.0, false));
        assert!(!clear_fake(Verdict::Transcoded, 19_940.0, 39.6, false), "20 kHz may be a master's own filter");
        assert!(!clear_fake(Verdict::Transcoded, 20_200.0, 48.8, false));
        assert!(clear_fake(Verdict::Transcoded, 16_100.0, 44.2, false), "an MP3 128's cliff");
        assert!(clear_fake(Verdict::Transcoded, 18_200.0, 31.6, false));
        assert!(!clear_fake(Verdict::Transcoded, 16_785.0, 35.0, true), "hi-res needs a steeper cliff");
        assert!(!clear_fake(Verdict::Genuine, 22_000.0, 0.0, false));
    }

    #[test]
    fn the_panel_says_it_in_plain_words() {
        let fake = check(Verdict::Transcoded, 16_000.0, None);
        assert!(verdict_text(&Codec::Flac, None, &fake).starts_with("Не настоящий lossless"));
        assert!(verdict_text(&Codec::Flac, None, &fake).contains("MP3 ~128 кбит/с"));
        let real = check(Verdict::Genuine, 22_050.0, None);
        assert!(verdict_text(&Codec::Flac, None, &real).starts_with("Настоящий lossless"));
        assert_eq!(cutoff_text(&real).as_deref(), Some("до 22,0 кГц, без среза"));
        assert_eq!(cutoff_text(&fake).as_deref(), Some("до 16,0 кГц"));
        let conv = check(Verdict::Transcoded, 19_600.0, Some(Codec::Flac));
        assert!(verdict_text(&Codec::Mp3, Some(320), &conv).starts_with("Сделан из поддельного FLAC"));
        let opus = check(Verdict::Genuine, 22_050.0, Some(Codec::Flac));
        assert_eq!(verdict_text(&Codec::Opus, Some(160), &opus), "Сделан из настоящего FLAC");
        let up = check(Verdict::Upsampled, 22_000.0, None);
        assert!(verdict_text(&Codec::Flac, None, &up).starts_with("Lossless, но не Hi-Res"));
        let unknown = check(Verdict::Unknown, 0.0, None);
        assert!(verdict_text(&Codec::Flac, None, &unknown).starts_with("Не определить"));
        assert_eq!(cutoff_text(&unknown), None);
        let mp3 = check(Verdict::Transcoded, 20_100.0, None);
        assert_eq!(verdict_text(&Codec::Mp3, Some(320), &mp3), "Полоса соответствует формату");
    }

    #[test]
    fn a_fake_scores_as_what_it_really_is() {
        assert_eq!(fake_score(SoundOrigin::Lossy { kbps: 256 }, 19_600.0), score::FAKE_LOSSLESS);
        assert!(fake_score(SoundOrigin::Lossy { kbps: 128 }, 16_000.0) < score::FAKE_LOSSLESS);
        assert_eq!(fake_score(SoundOrigin::Unknown, 16_000.0), score::FAKE_LOSSLESS);
        assert!(fake_score(SoundOrigin::Lossy { kbps: 256 }, 19_600.0) < score::MP3_V0);
        // A cut a real master may have too: never below an MP3 320, never
        // above a real CD.
        assert_eq!(fake_score(SoundOrigin::Lossy { kbps: 320 }, 20_186.0), score::MP3_320);
        assert_eq!(fake_score(SoundOrigin::Lossy { kbps: 256 }, 19_800.0), score::MP3_320);
        assert!(fake_score(SoundOrigin::Lossy { kbps: 320 }, 20_186.0) < score::CD_LOSSLESS);
        assert!(fake_score(SoundOrigin::Lossy { kbps: 256 }, 19_799.0) < score::MP3_V0);
        assert_eq!(Mp3Target::for_origin(SoundOrigin::Lossy { kbps: 320 }), Mp3Target::Cbr320);
        assert_eq!(Mp3Target::for_origin(SoundOrigin::Lossy { kbps: 256 }), Mp3Target::Cbr320);
        assert_eq!(Mp3Target::for_origin(SoundOrigin::Lossy { kbps: 192 }), Mp3Target::V0);
        assert_eq!(Mp3Target::for_origin(SoundOrigin::Unknown), Mp3Target::Cbr320);
    }

    #[test]
    fn the_spectrum_survives_storage_and_downsampling() {
        let flat = vec![-50.0f32; 2049];
        let d = downsample_db(&flat, SPECTRUM_POINTS);
        assert_eq!(d.len(), SPECTRUM_POINTS);
        assert!(d.iter().all(|x| (x + 50.0).abs() < 0.01));
        let back = spectrum_from_blob(&spectrum_to_blob(&[-12.34, -110.0, 0.0]));
        assert_eq!(back, vec![-12.3, -110.0, 0.0]);
        assert!(downsample_db(&[], 10).is_empty());
    }

    /// «Really better» in one place: an MP3 gets a genuine lossless copy,
    /// a fake FLAC too; our Opus of a genuine FLAC and a genuine FLAC get
    /// nothing, whatever is offered.
    #[test]
    fn really_better_is_one_rule() {
        let mp3_320 = quality_score(&Codec::Mp3, Some(320), None, None, false);
        assert!(really_better(&Codec::Mp3, mp3_320, None, score::CD_LOSSLESS), "MP3 → genuine lossless");
        assert!(really_better(&Codec::Mp3, 500, None, score::HI_RES_LOSSLESS));
        assert!(!really_better(&Codec::Mp3, mp3_320, None, mp3_320), "the same is not better");
        assert!(can_get_better(&Codec::Mp3, 500, None));

        // Our Opus of a genuine FLAC: ranked as the FLAC, its check of it.
        let genuine = check(Verdict::Genuine, 22_050.0, Some(Codec::Flac)).summary();
        for (rank, check) in [(score::CD_LOSSLESS, Some(&genuine)), (score::HI_RES_LOSSLESS, Some(&genuine)), (score::CD_LOSSLESS, None)] {
            assert!(nothing_better(&Codec::Opus, rank, check));
            for offer in [score::MP3_320, score::CD_LOSSLESS, score::HI_RES_LOSSLESS, i32::MAX] {
                assert!(!really_better(&Codec::Opus, rank, check, offer), "Opus of a FLAC → {offer}");
            }
            assert!(!can_get_better(&Codec::Opus, rank, check));
        }
        // An Opus at its own bitrate is a lossy file like any other.
        assert!(really_better(&Codec::Opus, score::OPUS_160, None, score::CD_LOSSLESS));

        // A fake FLAC (ranked as what it holds) → genuine lossless: better.
        let fake = check(Verdict::Transcoded, 16_000.0, None).summary();
        let fake_rank = fake_score(SoundOrigin::Lossy { kbps: 128 }, 16_000.0);
        assert!(really_better(&Codec::Flac, fake_rank, Some(&fake), score::CD_LOSSLESS));
        assert!(can_get_better(&Codec::Flac, fake_rank, Some(&fake)));
        // A doubt (ranked as an MP3 320) still gets a genuine one.
        let doubt = check(Verdict::Transcoded, 20_186.0, None).summary();
        assert!(really_better(&Codec::Flac, score::MP3_320, Some(&doubt), score::CD_LOSSLESS));
        // An MP3 made from a fake FLAC too.
        let from_fake = check(Verdict::Transcoded, 16_000.0, Some(Codec::Flac)).summary();
        assert!(really_better(&Codec::Mp3, fake_rank, Some(&from_fake), score::CD_LOSSLESS));

        // A genuine FLAC: nothing, not even hi-res.
        let clean = check(Verdict::Genuine, 22_050.0, None).summary();
        assert!(nothing_better(&Codec::Flac, score::CD_LOSSLESS, Some(&clean)));
        assert!(!really_better(&Codec::Flac, score::CD_LOSSLESS, Some(&clean), score::HI_RES_LOSSLESS));
        assert!(!can_get_better(&Codec::Flac, score::CD_LOSSLESS, Some(&clean)));
        // One not checked yet may be a fake: only a higher rank beats it.
        assert!(!nothing_better(&Codec::Flac, score::CD_LOSSLESS, None));
        assert!(really_better(&Codec::Flac, score::CD_LOSSLESS, None, score::HI_RES_LOSSLESS));
        assert!(!really_better(&Codec::Flac, score::CD_LOSSLESS, None, score::CD_LOSSLESS));
        assert!(!can_get_better(&Codec::Flac, score::CD_LOSSLESS, None));
    }

    #[test]
    fn english_labels_verdicts_and_cutoffs() {
        let plain = check(Verdict::Transcoded, 16_000.0, None);
        assert_eq!(row_label_in(&Codec::Flac, Some(&plain.summary()), Locale::En), "FLAC from MP3 ~128");
        assert_eq!(label_in(&Codec::Flac, Some(1000), Some(&plain.summary()), Locale::En), "FLAC from MP3 ~128");
        assert!(row_label_mp3_inside(&row_label_in(&Codec::Flac, Some(&plain.summary()), Locale::En)));
        assert!(row_label_mp3_inside("WAV from lossy"));
        assert!(!row_label_mp3_inside("FLAC, possibly from MP3 320"));
        assert_eq!(
            verdict_text_in(&Codec::Flac, None, &plain, Locale::En),
            "Not true lossless: transcoded from a lossy file, looks like MP3 ~128 kbps"
        );
        assert_eq!(cutoff_text_in(&plain, Locale::En).as_deref(), Some("up to 16.0 kHz"));

        let no_kbps = CheckSummary {
            verdict: Verdict::Transcoded,
            origin: SoundOrigin::Unknown,
            made_from: None,
            cutoff_hz: Some(16_000),
        };
        assert_eq!(row_label_in(&Codec::Wav, Some(&no_kbps), Locale::En), "WAV from lossy");

        let maybe = check(Verdict::Transcoded, 20_186.0, None);
        assert_eq!(row_label_in(&Codec::Flac, Some(&maybe.summary()), Locale::En), "FLAC");
        assert_eq!(
            label_in(&Codec::Flac, Some(1000), Some(&maybe.summary()), Locale::En),
            "FLAC, possibly from MP3 320"
        );
        assert!(!row_label_mp3_inside(&row_label_in(&Codec::Flac, Some(&maybe.summary()), Locale::En)));
        assert_eq!(
            verdict_text_in(&Codec::Flac, None, &maybe, Locale::En),
            "Possibly transcoded from MP3 ~320 kbps; a 20.2 kHz cutoff also occurs in genuine masters"
        );

        let unsure_unknown = CheckSummary {
            verdict: Verdict::Transcoded,
            origin: SoundOrigin::Unknown,
            made_from: None,
            cutoff_hz: Some(20_186),
        };
        assert_eq!(
            label_in(&Codec::Flac, None, Some(&unsure_unknown), Locale::En),
            "FLAC, possibly from lossy"
        );

        let conv = check(Verdict::Transcoded, 19_600.0, Some(Codec::Flac));
        assert_eq!(label_in(&Codec::Mp3, Some(320), Some(&conv.summary()), Locale::En), "MP3 320 from fake FLAC");
        assert_eq!(row_label_in(&Codec::Mp3, Some(&conv.summary()), Locale::En), "MP3 from fake FLAC");
        assert!(!row_label_mp3_inside(&row_label_in(&Codec::Mp3, Some(&conv.summary()), Locale::En)));
        assert_eq!(
            verdict_text_in(&Codec::Mp3, Some(320), &conv, Locale::En),
            "Encoded from fake FLAC: contained lossy audio, looks like MP3 ~256 kbps"
        );

        let conv_unsure = check(Verdict::Transcoded, 20_100.0, Some(Codec::Flac));
        assert_eq!(
            label_in(&Codec::Mp3, Some(320), Some(&conv_unsure.summary()), Locale::En),
            "MP3 320 from possibly fake FLAC"
        );
        assert_eq!(row_label_in(&Codec::Mp3, Some(&conv_unsure.summary()), Locale::En), "MP3");
        assert_eq!(
            verdict_text_in(&Codec::Mp3, Some(320), &conv_unsure, Locale::En),
            "Encoded from FLAC, possibly transcoded from MP3 ~320 kbps; a 20.1 kHz cutoff also occurs in genuine masters"
        );

        let real = check(Verdict::Genuine, 22_050.0, None);
        assert_eq!(row_label_in(&Codec::Flac, Some(&real.summary()), Locale::En), "FLAC");
        assert_eq!(verdict_text_in(&Codec::Flac, None, &real, Locale::En), "True lossless: full frequency band");
        assert_eq!(cutoff_text_in(&real, Locale::En).as_deref(), Some("up to 22.0 kHz, no cutoff"));

        let opus = check(Verdict::Genuine, 22_050.0, Some(Codec::Flac));
        assert_eq!(label_in(&Codec::Opus, Some(160), Some(&opus.summary()), Locale::En), "Opus 160");
        assert_eq!(verdict_text_in(&Codec::Opus, Some(160), &opus, Locale::En), "Encoded from genuine FLAC");

        let up = check(Verdict::Upsampled, 22_000.0, None);
        assert_eq!(
            verdict_text_in(&Codec::Flac, None, &up, Locale::En),
            "Lossless, not Hi-Res: CD-quality audio upsampled to a higher rate"
        );

        let unknown = check(Verdict::Unknown, 0.0, None);
        assert_eq!(
            verdict_text_in(&Codec::Flac, None, &unknown, Locale::En),
            "Inconclusive: recording is too quiet or too short"
        );
        assert_eq!(cutoff_text_in(&unknown, Locale::En), None);

        let mp3 = check(Verdict::Transcoded, 20_100.0, None);
        assert_eq!(verdict_text_in(&Codec::Mp3, Some(320), &mp3, Locale::En), "Spectrum matches format");
        assert_eq!(khz_in(16_100.0, Locale::En), "16.1 kHz");
        assert_eq!(khz_in(16_100.0, Locale::Ru), "16,1 кГц");
    }
}
