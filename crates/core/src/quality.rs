//! What the spectrum check found about a file, and how the player names a
//! sound without passing a fake off as lossless.
//!
//! The check itself lives in the importer (`nikgob_import::spectrum`); its
//! result is kept per source in the library ([`QualityCheck`]) and read
//! back here into labels: «FLAC из MP3 ~320» for a FLAC made from an MP3,
//! «MP3 320 из поддельного FLAC» for one converted after the check, and
//! the plain words of the track's quality panel.

use std::fmt;

use serde::{Deserialize, Serialize};

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
        }
    }
}

/// What a track row needs of a check: the label, without the spectrum.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSummary {
    pub verdict: Verdict,
    pub origin: SoundOrigin,
    pub made_from: Option<Codec>,
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

/// A fake so plain that turning it into an MP3 loses nothing for sure: a
/// lossy encoder's cliff (a steep drop where MP3s cut, 15 kHz and up), and
/// a steeper one still for a file that came as hi-res. Anything less is
/// labelled, never converted.
pub fn clear_fake(verdict: Verdict, cutoff_hz: f32, steepness_db: f32, hi_res: bool) -> bool {
    plausible(verdict, cutoff_hz) == Verdict::Transcoded
        && cutoff_hz >= 15_000.0
        && steepness_db >= if hi_res { 40.0 } else { 30.0 }
}

impl CheckSummary {
    /// `verdict|origin|made_from`, as the library's track rows select it.
    pub fn encode(&self) -> String {
        format!(
            "{}|{}|{}",
            self.verdict.as_str(),
            self.origin.as_str(),
            self.made_from.as_ref().map_or("", Codec::as_str)
        )
    }

    pub fn decode(s: &str) -> Option<Self> {
        let mut it = s.split('|');
        let verdict = Verdict::parse(it.next()?);
        let origin = SoundOrigin::parse(it.next()?);
        let made_from = it.next().filter(|m| !m.is_empty()).map(Codec::parse);
        Some(Self { verdict, origin, made_from })
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

/// `quality_score` of a lossless file found transcoded: what its content
/// really is (an MP3 of the estimated bitrate), and never above
/// [`score::FAKE_LOSSLESS`], below MP3 V0.
pub fn fake_score(origin: SoundOrigin) -> i32 {
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

/// The codec as a list row shows it, honest about what the content is:
/// «FLAC из MP3 ~320», «MP3 из поддельного FLAC», «MP3 из ~128», else the
/// codec («FLAC», «Opus»).
pub fn row_label(codec: &Codec, check: Option<&CheckSummary>) -> String {
    label(codec, None, check)
}

/// [`row_label`] with the bitrate where it says something: «MP3 320 из
/// поддельного FLAC», «MP3 320 из ~128», «FLAC из MP3 ~320», «Opus 160».
pub fn label(codec: &Codec, bitrate: Option<u32>, check: Option<&CheckSummary>) -> String {
    let name = codec_name(codec);
    let with_rate = match bitrate.filter(|_| !codec.is_lossless()) {
        Some(b) => format!("{name} {b}"),
        None => name.clone(),
    };
    // A lossy file's own check says nothing (see [`check_counts`]).
    let Some(c) = check.filter(|c| check_counts(codec, c.made_from.as_ref())) else { return with_rate };
    if c.fake(codec) {
        return match c.origin.lossy_kbps() {
            Some(k) => format!("{name} из MP3 ~{k}"),
            None => format!("{name} из сжатого"),
        };
    }
    if c.from_fake(codec) {
        let from = c.made_from.as_ref().map(codec_name).unwrap_or_default();
        return format!("{with_rate} из поддельного {from}");
    }
    with_rate
}

/// "16,1 кГц".
pub fn khz(hz: f32) -> String {
    format!("{:.1} кГц", hz / 1000.0).replace('.', ",")
}

/// The verdict in plain words for the quality panel. `codec` and
/// `bitrate` are the source's own; the check is one that counts
/// ([`check_counts`]).
pub fn verdict_text(codec: &Codec, bitrate: Option<u32>, check: &QualityCheck) -> String {
    let _ = bitrate;
    let s = check.summary();
    let like = |o: SoundOrigin| match o.lossy_kbps() {
        Some(k) => format!("похоже на MP3 ~{k} кбит/с"),
        None => "сжатый источник".to_owned(),
    };
    if s.fake(codec) {
        return format!("Не настоящий lossless: сделан из сжатого файла, {}", like(check.origin));
    }
    if s.from_fake(codec) {
        return format!(
            "Сделан из поддельного {}: внутри был сжатый звук, {}",
            check.made_from.as_ref().map(codec_name).unwrap_or_default(),
            like(check.origin)
        );
    }
    match (s.verdict, &check.made_from) {
        (Verdict::Unknown, _) => "Не определить: запись слишком тихая или короткая".into(),
        (Verdict::Upsampled, _) => "Lossless, но не Hi-Res: внутри качество CD, растянутое до большей частоты".into(),
        (Verdict::Genuine, Some(from)) => format!("Сделан из настоящего {}", codec_name(from)),
        (Verdict::Genuine, None) if codec.is_lossless() => "Настоящий lossless: полоса без среза".into(),
        _ => "Полоса соответствует формату".into(),
    }
}

/// «Полоса до 16,1 кГц»; for a full band, «Полоса до 22,0 кГц, без среза».
pub fn cutoff_text(check: &QualityCheck) -> Option<String> {
    if check.verdict == Verdict::Unknown || check.cutoff_hz <= 0.0 {
        return None;
    }
    let nyquist = check.sample_rate as f32 / 2.0;
    Some(if check.cutoff_hz >= nyquist * 0.98 {
        format!("до {}, без среза", khz(nyquist))
    } else {
        format!("до {}", khz(check.cutoff_hz))
    })
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
        assert_eq!(s.encode(), "transcoded|lossy:128|flac");
        assert_eq!(CheckSummary::decode(&s.encode()), Some(s));
        let plain = check(Verdict::Genuine, 22_050.0, None).summary();
        assert_eq!(CheckSummary::decode(&plain.encode()), Some(plain));
        assert_eq!(CheckSummary::decode(""), None);
    }

    #[test]
    fn a_fake_is_never_labelled_lossless() {
        let fake = check(Verdict::Transcoded, 20_100.0, None).summary();
        assert_eq!(row_label(&Codec::Flac, Some(&fake)), "FLAC из MP3 ~320");
        let low = check(Verdict::Transcoded, 16_000.0, None).summary();
        assert_eq!(label(&Codec::Flac, Some(1000), Some(&low)), "FLAC из MP3 ~128");
        // Converted after the check.
        let conv = check(Verdict::Transcoded, 20_100.0, Some(Codec::Flac)).summary();
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
        assert!(clear_fake(Verdict::Transcoded, 19_940.0, 39.6, false));
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
        let conv = check(Verdict::Transcoded, 20_100.0, Some(Codec::Flac));
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
        assert_eq!(fake_score(SoundOrigin::Lossy { kbps: 320 }), score::FAKE_LOSSLESS);
        assert!(fake_score(SoundOrigin::Lossy { kbps: 128 }) < score::FAKE_LOSSLESS);
        assert_eq!(fake_score(SoundOrigin::Unknown), score::FAKE_LOSSLESS);
        assert!(fake_score(SoundOrigin::Lossy { kbps: 320 }) < score::MP3_V0);
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
}
