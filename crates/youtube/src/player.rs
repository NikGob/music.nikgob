//! Resolving a video id to an audio stream URL.
//!
//! The player is asked for by the visionOS client first: it answers with
//! plain stream URLs (no signature deobfuscation, no player JavaScript) that
//! serve the whole file without a PO token. This is what yt-dlp uses when it
//! has no JavaScript runtime. RustyPipe 0.11.4 cannot do this itself: its
//! deobfuscator no longer understands YouTube's player code (so the TV and
//! web clients fail), and the iOS client's URLs stop after the first ~1 MB
//! without a PO token. RustyPipe's own player stays as the fallback (it
//! improves if a `rustypipe-botguard` binary is configured or RustyPipe gets
//! fixed).

use std::time::Duration;

use nikgob_core::streaming::{StreamError, format};
use rustypipe::client::RustyPipe;
use serde::Deserialize;
use serde_json::json;

use crate::map_err;
use crate::range::Resolved;

const PLAYER_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const VISIONOS_NAME: &str = "VISIONOS";
const VISIONOS_ID: &str = "101";
const VISIONOS_VERSION: &str = "1.02";
const VISIONOS_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 \
                           (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

/// One audio format, whichever client it came from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Format {
    pub itag: u32,
    pub url: Option<String>,
    /// `true` for Opus in WebM, `false` for AAC in MP4; other codecs are
    /// left out.
    pub opus: bool,
    pub bitrate: u32,
    pub len: u64,
    pub duration_ms: Option<u64>,
    pub loudness_db: Option<f32>,
    /// The original audio track (videos may have dubbed ones too).
    pub default_track: bool,
    pub drm: bool,
}

/// Itags in order of preference: Opus 160k / 70k / 50k, AAC 256k / 128k.
const ITAGS: &[u32] = &[251, 250, 249, 141, 140];

/// The best usable format: Opus WebM 251, else other Opus, else AAC 141 /
/// 140; never DRM, only the original audio track.
pub(crate) fn pick(formats: &[Format]) -> Option<&Format> {
    let rank = |f: &Format| {
        let itag = ITAGS
            .iter()
            .position(|&i| i == f.itag)
            .unwrap_or(ITAGS.len());
        (itag, !f.opus, std::cmp::Reverse(f.bitrate))
    };
    formats
        .iter()
        .filter(|f| f.url.is_some() && !f.drm && f.default_track && f.len > 0)
        .min_by_key(|f| rank(f))
}

/// YouTube's `loudness_db` is how far the track is above its reference
/// level; YouTube turns loud tracks down by that much and never turns quiet
/// ones up.
pub(crate) fn gain_from_loudness(loudness_db: Option<f32>) -> Option<f32> {
    loudness_db
        .filter(|l| l.is_finite())
        .map(|l| if l > 0.0 { -l } else { 0.0 })
}

fn resolved(f: &Format, fallback_duration: Option<Duration>) -> Result<Resolved, StreamError> {
    Ok(Resolved {
        url: f.url.clone().ok_or(StreamError::Unavailable)?,
        len: f.len,
        format: if f.opus { format::WEBM } else { format::M4A },
        duration: f
            .duration_ms
            .map(Duration::from_millis)
            .or(fallback_duration),
        gain_db: gain_from_loudness(f.loudness_db),
        itag: f.itag,
        bitrate: f.bitrate,
    })
}

// --- the visionOS player answer ---------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayerAnswer {
    playability_status: Option<Playability>,
    streaming_data: Option<StreamingData>,
    video_details: Option<VideoDetails>,
}

#[derive(Deserialize)]
struct Playability {
    status: String,
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamingData {
    #[serde(default)]
    adaptive_formats: Vec<RawFormat>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoDetails {
    length_seconds: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFormat {
    itag: u32,
    url: Option<String>,
    mime_type: String,
    #[serde(default)]
    bitrate: u32,
    content_length: Option<String>,
    approx_duration_ms: Option<String>,
    loudness_db: Option<f32>,
    audio_track: Option<RawAudioTrack>,
    #[serde(default)]
    drm_families: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAudioTrack {
    #[serde(default)]
    audio_is_default: bool,
}

/// Why the visionOS player gave nothing.
#[derive(Debug, PartialEq)]
pub(crate) enum PlayerError {
    /// "Sign in to confirm you're not a bot": try fresh visitor data.
    Bot,
    Other(StreamError),
}

/// Audio formats and the video length from a player answer.
pub(crate) fn parse_player(json: &str) -> Result<(Vec<Format>, Option<Duration>), PlayerError> {
    let a: PlayerAnswer = serde_json::from_str(json)
        .map_err(|e| PlayerError::Other(StreamError::Parse(format!("player: {e}"))))?;
    match a.playability_status {
        Some(p) if p.status == "OK" => {}
        Some(p) => {
            let reason = p.reason.unwrap_or_default();
            let e = playability_error(&p.status, &reason);
            log::info!("youtube: player: {} «{reason}» → {e:?}", p.status);
            return Err(e);
        }
        None => {
            return Err(PlayerError::Other(StreamError::Parse(
                "player: no status".into(),
            )));
        }
    }
    let duration = a
        .video_details
        .and_then(|d| d.length_seconds?.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs);
    let formats = a
        .streaming_data
        .map(|s| s.adaptive_formats)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| {
            let opus = if f.mime_type.starts_with("audio/webm") && f.mime_type.contains("opus") {
                true
            } else if f.mime_type.starts_with("audio/mp4") && f.mime_type.contains("mp4a") {
                false
            } else {
                return None;
            };
            Some(Format {
                itag: f.itag,
                url: f.url,
                opus,
                bitrate: f.bitrate,
                len: f.content_length.and_then(|l| l.parse().ok()).unwrap_or(0),
                duration_ms: f.approx_duration_ms.and_then(|d| d.parse().ok()),
                loudness_db: f.loudness_db,
                default_track: f.audio_track.is_none_or(|t| t.audio_is_default),
                drm: !f.drm_families.is_empty(),
            })
        })
        .collect();
    Ok((formats, duration))
}

/// What a refused player answer means. The request asks for English, so
/// the reasons can be told apart: "Sign in to confirm you're not a bot" (a
/// fresh visitor id helps), "Sign in to confirm your age" / "inappropriate
/// for some users" (the age gate: only a login helps), "not available in
/// your country" (a region or label block).
pub(crate) fn playability_error(status: &str, reason: &str) -> PlayerError {
    let r = reason.to_lowercase();
    if r.contains("bot") {
        return PlayerError::Bot;
    }
    if r.contains("your age") || r.contains("age-restricted") || r.contains("inappropriate") {
        return PlayerError::Other(StreamError::AgeRestricted);
    }
    if r.contains("country") || r.contains("region") || r.contains("location") {
        return PlayerError::Other(StreamError::RegionBlocked);
    }
    match status {
        // A sign-in wall without a stated reason: a fresh visitor id may do.
        "LOGIN_REQUIRED" => PlayerError::Bot,
        _ => PlayerError::Other(StreamError::Unavailable),
    }
}

async fn visionos(
    http: &reqwest::Client,
    visitor_data: &str,
    id: &str,
    _hl: &str,
    gl: &str,
) -> Result<(Vec<Format>, Option<Duration>), PlayerError> {
    let body = json!({
        "context": { "client": {
            "clientName": VISIONOS_NAME,
            "clientVersion": VISIONOS_VERSION,
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "osName": "visionOS",
            "osVersion": "26.5.23O471",
            // English reasons (see `playability_error`); metadata does not
            // come from here.
            "hl": "en",
            "gl": gl,
            "visitorData": visitor_data,
        }},
        "videoId": id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let resp = http
        .post(PLAYER_URL)
        .header(reqwest::header::USER_AGENT, VISIONOS_UA)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-YouTube-Client-Name", VISIONOS_ID)
        .header("X-YouTube-Client-Version", VISIONOS_VERSION)
        .header("X-Goog-Visitor-Id", visitor_data)
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| PlayerError::Other(StreamError::Network(e.without_url().to_string())))?;
    let status = resp.status().as_u16();
    if status == 429 {
        return Err(PlayerError::Other(StreamError::RateLimited {
            retry_after: None,
        }));
    }
    if status != 200 {
        return Err(PlayerError::Other(StreamError::Http { status }));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| PlayerError::Other(StreamError::Network(e.without_url().to_string())))?;
    parse_player(&text)
}

/// RustyPipe's player: with the user's login first when there is one (the
/// age gate opens for a signed-in adult; cookies sign in the web clients),
/// else its default clients.
async fn rustypipe_player(
    rp: &RustyPipe,
    id: &str,
) -> Result<rustypipe::model::VideoPlayer, rustypipe::error::Error> {
    use rustypipe::client::ClientType;
    let q = rp.query();
    let signed_in: Vec<ClientType> = [ClientType::DesktopMusic, ClientType::Desktop, ClientType::Tv]
        .into_iter()
        .filter(|c| q.auth_enabled(*c))
        .collect();
    if !signed_in.is_empty() {
        match q.clone().authenticated().player_from_clients(id, &signed_in).await {
            Ok(p) => {
                log::debug!("youtube: {id}: player with login via {:?}", p.client_type);
                return Ok(p);
            }
            Err(e) => log::info!("youtube: {id}: player with login failed: {e}"),
        }
    }
    q.player(id).await
}

/// The best audio stream of a video: visionOS first, RustyPipe's player if
/// that fails.
pub(crate) async fn resolve(
    rp: &RustyPipe,
    http: &reqwest::Client,
    id: &str,
    hl: &str,
    gl: &str,
) -> Result<Resolved, StreamError> {
    let mut fresh = false;
    let first_err = loop {
        let vd = match rp.query().get_visitor_data(fresh).await {
            Ok(vd) => vd,
            Err(e) => break map_err(e),
        };
        match visionos(http, &vd, id, hl, gl).await {
            Ok((formats, duration)) => match pick(&formats) {
                Some(f) => {
                    log::debug!(
                        "youtube: {id}: itag {} via visionOS, {} bytes",
                        f.itag,
                        f.len
                    );
                    return resolved(f, duration);
                }
                None => break StreamError::Unavailable,
            },
            Err(PlayerError::Bot) if !fresh => {
                rp.query().remove_visitor_data(&vd);
                fresh = true;
            }
            Err(PlayerError::Bot) => break StreamError::RateLimited { retry_after: None },
            Err(PlayerError::Other(e)) => break e,
        }
    };
    log::info!("youtube: {id}: visionOS player failed ({first_err}), trying RustyPipe's");
    let player = rustypipe_player(rp, id).await.map_err(|e| {
        log::info!("youtube: {id}: RustyPipe player: {e}");
        // The more telling reason of the two.
        match (&first_err, crate::map_err(e)) {
            (StreamError::AgeRestricted | StreamError::RegionBlocked, _) => first_err.clone(),
            (_, e @ (StreamError::AgeRestricted | StreamError::RegionBlocked)) => e,
            _ => first_err.clone(),
        }
    })?;
    let formats: Vec<Format> = player
        .audio_streams
        .iter()
        .filter_map(|s| {
            use rustypipe::model::{AudioCodec, AudioFormat};
            let opus = match (s.format, s.codec) {
                (AudioFormat::Webm, AudioCodec::Opus) => true,
                (AudioFormat::M4a, AudioCodec::Mp4a) => false,
                _ => return None,
            };
            Some(Format {
                itag: s.itag,
                url: Some(s.url.clone()),
                opus,
                bitrate: s.bitrate,
                len: s.size,
                duration_ms: s.duration_ms.map(u64::from),
                loudness_db: s.loudness_db,
                default_track: s.track.as_ref().is_none_or(|t| t.is_default),
                drm: s.drm_track_type.is_some(),
            })
        })
        .collect();
    let f = pick(&formats).ok_or(first_err)?;
    log::debug!(
        "youtube: {id}: itag {} via {:?}, {} bytes",
        f.itag,
        player.client_type,
        f.len
    );
    let duration = (player.details.duration > 0)
        .then(|| Duration::from_secs(u64::from(player.details.duration)));
    resolved(f, duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_turns_loud_tracks_down_only() {
        assert_eq!(gain_from_loudness(Some(6.5)), Some(-6.5));
        assert_eq!(gain_from_loudness(Some(-3.0)), Some(0.0));
        assert_eq!(gain_from_loudness(None), None);
        assert_eq!(gain_from_loudness(Some(f32::NAN)), None);
    }

    fn answer(formats: serde_json::Value) -> String {
        json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {"lengthSeconds": "166"},
            "streamingData": {"adaptiveFormats": formats},
        })
        .to_string()
    }

    fn raw(itag: u32, mime: &str, bitrate: u32) -> serde_json::Value {
        json!({"itag": itag, "url": format!("https://x/{itag}"), "mimeType": mime,
               "bitrate": bitrate, "contentLength": "2887956", "approxDurationMs": "166061",
               "loudnessDb": 2.5})
    }

    #[test]
    fn parses_the_player_and_prefers_opus_251() {
        let json = answer(json!([
            raw(137, "video/mp4; codecs=\"avc1.640028\"", 4_000_000),
            raw(140, "audio/mp4; codecs=\"mp4a.40.2\"", 130_000),
            raw(249, "audio/webm; codecs=\"opus\"", 50_000),
            raw(251, "audio/webm; codecs=\"opus\"", 160_000),
        ]));
        let (formats, duration) = parse_player(&json).unwrap();
        assert_eq!(formats.len(), 3, "video formats are left out");
        assert_eq!(duration, Some(Duration::from_secs(166)));
        let f = pick(&formats).unwrap();
        assert_eq!((f.itag, f.opus, f.len), (251, true, 2_887_956));
        let r = resolved(f, duration).unwrap();
        assert_eq!(r.format, format::WEBM);
        assert_eq!(r.duration, Some(Duration::from_millis(166_061)));
        assert_eq!(r.gain_db, Some(-2.5));
        assert_eq!(pick(&formats[..1]).unwrap().itag, 140);
        assert!(pick(&[]).is_none());
    }

    #[test]
    fn skips_dubbed_drm_and_url_less_formats() {
        let mut dubbed = raw(251, "audio/webm; codecs=\"opus\"", 160_000);
        dubbed["audioTrack"] = json!({"audioIsDefault": false});
        let mut drm = raw(141, "audio/mp4; codecs=\"mp4a.40.2\"", 256_000);
        drm["drmFamilies"] = json!(["WIDEVINE"]);
        let mut cipher = raw(250, "audio/webm; codecs=\"opus\"", 70_000);
        cipher.as_object_mut().unwrap().remove("url");
        let ok = raw(140, "audio/mp4; codecs=\"mp4a.40.2\"", 130_000);
        let (formats, _) = parse_player(&answer(json!([dubbed, drm, cipher, ok]))).unwrap();
        assert_eq!(pick(&formats).unwrap().itag, 140);
    }

    #[test]
    fn playability_failures() {
        let bot = json!({"playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "bot"}});
        assert_eq!(parse_player(&bot.to_string()), Err(PlayerError::Bot));
        let age = json!({"playabilityStatus": {"status": "LOGIN_REQUIRED",
            "reason": "Sign in to confirm your age"}});
        assert_eq!(parse_player(&age.to_string()), Err(PlayerError::Other(StreamError::AgeRestricted)));
        let region = json!({"playabilityStatus": {"status": "UNPLAYABLE",
            "reason": "The uploader has not made this video available in your country"}});
        assert_eq!(parse_player(&region.to_string()), Err(PlayerError::Other(StreamError::RegionBlocked)));
        let gone = json!({"playabilityStatus": {"status": "ERROR", "reason": "Video unavailable"}});
        assert_eq!(parse_player(&gone.to_string()), Err(PlayerError::Other(StreamError::Unavailable)));
        let gone = json!({"playabilityStatus": {"status": "ERROR", "reason": "removed"}});
        assert_eq!(
            parse_player(&gone.to_string()),
            Err(PlayerError::Other(StreamError::Unavailable))
        );
        assert!(matches!(
            parse_player("not json"),
            Err(PlayerError::Other(StreamError::Parse(_)))
        ));
    }
}
