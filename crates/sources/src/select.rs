//! The one quality policy over every source (docs/stage4.5.md): offers
//! that are the wished track or album ([`crate::matching::judge`]) are
//! ranked by quality first (Hi-Res FLAC > CD FLAC > … > lossy), then sure
//! before doubtful, then the user's order of the sources on «Источники»,
//! then how well they match and how available they are. The pick to
//! download without asking is the best sure lossless offer; lossy only
//! when it is better than a copy the item already has (the YouTube copy),
//! and otherwise the user chooses.

use nikgob_core::source::{Offer, SourceKey, Want};

use crate::matching::{Verdict, judge};
use crate::torrent::release_bonus;

/// An offer that is the wished thing, with how it matched.
#[derive(Clone, Debug)]
pub struct Ranked {
    pub offer: Offer,
    pub verdict: Verdict,
}

/// The offers that match `want`, best first; `order` is the user's order
/// of the sources (a source not in it goes last).
pub fn rank(offers: Vec<Offer>, want: &Want, order: &[SourceKey]) -> Vec<Ranked> {
    let place = |k: SourceKey| order.iter().position(|o| *o == k).unwrap_or(order.len());
    let mut out: Vec<(i32, bool, usize, f64, Ranked)> = offers
        .into_iter()
        .filter_map(|o| {
            let verdict = judge(&o, want)?;
            let extra = if o.source == SourceKey::Torrents { release_bonus(&o, want) } else { 0.0 };
            Some((o.quality(), verdict.sure, place(o.source), verdict.score + extra, Ranked { offer: o, verdict }))
        })
        .collect();
    out.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(b.3.total_cmp(&a.3))
            .then(b.4.offer.seeders.unwrap_or(0).cmp(&a.4.offer.seeders.unwrap_or(0)))
    });
    out.into_iter().map(|(.., r)| r).collect()
}

/// What to download without asking, from a [`rank`]ed list: the first sure
/// lossless offer; a sure lossy one only when it beats `copy` (the quality
/// of the copy the item has, `None` without one).
pub fn pick(ranked: &[Ranked], copy: Option<i32>) -> Option<usize> {
    if let Some(i) = ranked.iter().position(|r| r.verdict.sure && r.offer.lossless()) {
        return Some(i);
    }
    let copy = copy?;
    ranked.iter().position(|r| r.verdict.sure && r.offer.quality() > copy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::{Codec, score};
    use nikgob_core::source::{Format, OfferScope};

    fn offer(source: SourceKey, title: &str, format: Format, secs: u32) -> Offer {
        Offer {
            source,
            provider: String::new(),
            id: format!("{source:?}{title}"),
            title: title.into(),
            artist: (source != SourceKey::Torrents).then(|| "Eve".into()),
            track_title: (source != SourceKey::Torrents).then(|| "Underdog".into()),
            album: None,
            duration_ms: Some(secs * 1000),
            isrc: None,
            scope: if source == SourceKey::Torrents { OfferScope::Album } else { OfferScope::Track },
            format: Some(format),
            size_bytes: None,
            seeders: Some(5),
            info_url: None,
            handle: String::new(),
        }
    }

    fn want() -> Want {
        Want {
            artists: vec!["Eve".into()],
            title: Some("Underdog".into()),
            album: Some("Underdog".into()),
            duration_ms: Some(194_000),
            ..Want::default()
        }
    }

    #[test]
    fn quality_first_then_the_users_order() {
        let flac = Format::new(Codec::Flac);
        let hires = Format { bit_depth: Some(24), sample_rate: Some(96_000), ..Format::new(Codec::Flac) };
        let mp3 = Format { bitrate_kbps: Some(320), ..Format::new(Codec::Mp3) };
        let order = [SourceKey::Torrents, SourceKey::Qobuz, SourceKey::Tidal];
        let ranked = rank(
            vec![
                offer(SourceKey::Torrents, "Eve - Underdog [MP3 320]", mp3.clone(), 194),
                offer(SourceKey::Tidal, "Eve — Underdog", flac.clone(), 194),
                offer(SourceKey::Qobuz, "Eve — Underdog", flac.clone(), 194),
                offer(SourceKey::Qobuz, "Eve — Underdog (24-bit)", hires, 194),
                offer(SourceKey::Torrents, "Ado - Underdog [FLAC]", flac, 194),
            ],
            &want(),
            &order,
        );
        let got: Vec<(SourceKey, String)> = ranked.iter().map(|r| (r.offer.source, r.offer.format.as_ref().unwrap().label())).collect();
        assert_eq!(
            got,
            [
                (SourceKey::Qobuz, "FLAC 24/96".to_owned()),
                (SourceKey::Qobuz, "FLAC".to_owned()),
                (SourceKey::Tidal, "FLAC".to_owned()),
                (SourceKey::Torrents, "MP3 320".to_owned()),
            ],
            "another artist's torrent is out"
        );
        assert_eq!(pick(&ranked, None), Some(0));
        // Lossy only: asked, unless it beats the YouTube copy.
        let lossy: Vec<Ranked> = ranked.into_iter().filter(|r| !r.offer.lossless()).collect();
        assert_eq!(pick(&lossy, None), None);
    }

    #[test]
    fn a_lossy_offer_replaces_a_youtube_copy_at_once() {
        let mp3 = Format { bitrate_kbps: Some(320), ..Format::new(Codec::Mp3) };
        let ranked = rank(vec![offer(SourceKey::Qobuz, "Eve — Underdog", mp3, 194)], &want(), &[]);
        assert_eq!(pick(&ranked, Some(score::SAVED_STREAM)), Some(0));
        assert_eq!(pick(&ranked, Some(score::MP3_320)), None, "not better than the copy");
    }
}
