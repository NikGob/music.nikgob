//! Resolving a video id to an audio stream URL.
//!
//! The player is asked by our own innertube request, through a chain of
//! [`ROUTES`] (a client on a host). In October 2026 only the visionOS
//! client answers anonymously with plain stream URLs (no signature
//! deobfuscation, no player JavaScript) that serve the whole file without a
//! PO token; it is yt-dlp's only default client without a JavaScript
//! runtime. android_vr is kept last: its URLs serve the first ~1 MB and are
//! refused after that (403) since 2026-08-17, so its answer is taken only
//! if the end of the file answers too. The iOS client gives no URLs at all,
//! and the TV and web clients need the player JavaScript.
//!
//! The same client is asked on several hosts: music.youtube.com (where the
//! rest of YouTube Music goes), youtubei.googleapis.com and
//! www.youtube.com. A site blocker (a hosts entry) or a network that turns
//! one away fails the request at once ("error sending request"), and the
//! next host answers.
//!
//! A failure of a route is the client's or the host's, not the video's:
//! the route is benched for [`BENCH`] and the next one is asked at once;
//! the route that answered last is asked first (per session, see
//! [`Chain`]). A stream URL refused mid-track benches its client. The video
//! itself is refused only by the playability status (age gate, region,
//! gone). The whole resolve gives up after [`RESOLVE_BUDGET`].
//!
//! RustyPipe's player stays as the last resort: with the user's login for
//! the age gate (it needs RustyPipe's deobfuscator, which no longer
//! understands YouTube's player code), and anonymously (its iOS client's
//! URLs stop after the first ~1 MB without a PO token, so the end of the
//! file is checked first; it improves if a `rustypipe-botguard` binary is
//! configured or RustyPipe gets fixed). No anonymous client opens an
//! age-restricted video in October 2026: web_embedded says "unavailable",
//! the TV clients need the player JavaScript.

use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{StreamError, format};
use rustypipe::client::RustyPipe;
use serde::Deserialize;
use serde_json::json;

use crate::range::Resolved;

/// How long one resolve (the routes, then RustyPipe's player) may take.
pub(crate) const RESOLVE_BUDGET: Duration = Duration::from_secs(10);
/// One player request.
const PLAYER_TIMEOUT: Duration = Duration::from_secs(5);
/// The check that the end of a stream URL answers.
const TAIL_TIMEOUT: Duration = Duration::from_secs(4);
/// How long a failed route is asked only after the others.
pub(crate) const BENCH: Duration = Duration::from_secs(10 * 60);

/// A player client we can ask without JavaScript or a PO token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Client {
    VisionOs,
    AndroidVr,
}

impl Client {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Client::VisionOs => "visionOS",
            Client::AndroidVr => "android_vr",
        }
    }

    /// Its `context.client`, `X-YouTube-Client-Name`, version and user
    /// agent, as yt-dlp sends them.
    fn request(self, gl: &str, visitor_data: &str) -> (serde_json::Value, &'static str, &'static str, &'static str) {
        let (mut c, id, version, ua) = match self {
            Client::VisionOs => (
                json!({
                    "clientName": "VISIONOS",
                    "deviceMake": "Apple",
                    "deviceModel": "RealityDevice17,1",
                    "osName": "visionOS",
                    "osVersion": "26.5.23O471",
                }),
                "101",
                "1.02",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
            ),
            Client::AndroidVr => (
                json!({
                    "clientName": "ANDROID_VR",
                    "deviceMake": "Oculus",
                    "deviceModel": "Quest 3",
                    "androidSdkVersion": 32,
                    "osName": "Android",
                    "osVersion": "12L",
                }),
                "28",
                "1.65.10",
                "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; \
                 eureka-user Build/SQ3A.220605.009.A1) gzip",
            ),
        };
        c["clientVersion"] = json!(version);
        // English reasons (see `playability_error`); metadata does not come
        // from here.
        c["hl"] = json!("en");
        c["gl"] = json!(gl);
        if !visitor_data.is_empty() {
            c["visitorData"] = json!(visitor_data);
        }
        (c, id, version, ua)
    }

    /// Whether its URLs may stop serving after the first megabyte: the end
    /// of the file is checked before its answer is taken.
    fn check_tail(self) -> bool {
        self == Client::AndroidVr
    }
}

/// One way to ask the player: a client on a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub client: Client,
    pub host: &'static str,
}

/// In the order they are tried while none has answered yet.
pub(crate) const ROUTES: &[Route] = &[
    Route { client: Client::VisionOs, host: "music.youtube.com" },
    Route { client: Client::VisionOs, host: "youtubei.googleapis.com" },
    Route { client: Client::VisionOs, host: "www.youtube.com" },
    Route { client: Client::AndroidVr, host: "youtubei.googleapis.com" },
];

/// Which routes answered and failed lately (one per service, so per
/// session).
#[derive(Default)]
pub(crate) struct Chain {
    state: Mutex<ChainState>,
}

#[derive(Default)]
struct ChainState {
    /// The route that answered last.
    preferred: Option<usize>,
    /// Routes that failed, and when.
    benched: Vec<(usize, Instant)>,
}

impl Chain {
    fn lock(&self) -> MutexGuard<'_, ChainState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Indexes into [`ROUTES`] in the order to try them now: the one that
    /// answered last, the others in table order, the benched ones after
    /// those, benched longest ago first (a bench may be stale, and asking
    /// costs one quick request).
    pub(crate) fn order(&self) -> Vec<usize> {
        let mut st = self.lock();
        st.benched.retain(|(_, at)| at.elapsed() < BENCH);
        let benched: Vec<usize> = {
            let mut b = st.benched.clone();
            b.sort_by_key(|(_, at)| *at);
            b.into_iter().map(|(i, _)| i).collect()
        };
        let first = st.preferred.filter(|i| !benched.contains(i));
        let mut out: Vec<usize> = first.into_iter().collect();
        out.extend((0..ROUTES.len()).filter(|i| !benched.contains(i) && Some(*i) != first));
        out.extend(benched);
        out
    }

    pub(crate) fn worked(&self, i: usize) {
        let mut st = self.lock();
        st.benched.retain(|(b, _)| *b != i);
        if st.preferred != Some(i) {
            log::info!("youtube: the player is asked via {} on {} first now", ROUTES[i].client.label(), ROUTES[i].host);
        }
        st.preferred = Some(i);
    }

    pub(crate) fn bench(&self, i: usize, why: &str) {
        log::info!("youtube: player via {} on {} set aside: {why}", ROUTES[i].client.label(), ROUTES[i].host);
        let mut st = self.lock();
        st.benched.retain(|(b, _)| *b != i);
        st.benched.push((i, Instant::now()));
        if st.preferred == Some(i) {
            st.preferred = None;
        }
    }

    /// The stream URLs of `client` are refused: every route of it is
    /// benched.
    pub(crate) fn bench_client(&self, client: Client, why: &str) {
        for (i, r) in ROUTES.iter().enumerate() {
            if r.client == client {
                self.bench(i, why);
            }
        }
    }

    /// A URL of `r` was refused while streaming.
    pub(crate) fn refused(&self, r: &Resolved) {
        if let Some(i) = r.route {
            self.bench_client(ROUTES[i].client, "a stream URL of it was refused (403)");
        }
    }
}

/// What asking one route gave.
#[derive(Debug)]
pub(crate) enum Answer {
    Stream(Resolved),
    /// The player answered, but the video does not play for it.
    Player(PlayerError),
    /// Playable, but no format we can use (all ciphered, DRM, dubbed).
    NoFormats,
    /// The URL the player gave is refused at its end (see
    /// [`Client::check_tail`]).
    Refused,
    /// No answer (network, timeout, an HTTP error).
    Failed(StreamError),
}

/// Asks the routes in [`Chain::order`] until one gives a stream; `ask`
/// asks one route. A route that fails is benched and the next one asked at
/// once. A video the player refuses (age gate, region) is not asked again
/// elsewhere; one "unavailable" to a client is asked of the next client
/// ("made for kids" videos are unavailable to visionOS).
pub(crate) async fn run_chain<F, Fut>(chain: &Chain, id: &str, deadline: Instant, mut ask: F) -> Result<Resolved, StreamError>
where
    F: FnMut(usize) -> Fut,
    Fut: Future<Output = Answer>,
{
    let mut err: Option<StreamError> = None;
    let mut skip: Vec<Client> = Vec::new();
    for i in chain.order() {
        let route = ROUTES[i];
        if skip.contains(&route.client) {
            continue;
        }
        if Instant::now() >= deadline {
            log::info!("youtube: {id}: out of time for the player");
            break;
        }
        let label = route.client.label();
        match ask(i).await {
            Answer::Stream(r) => {
                log::debug!("youtube: {id}: itag {} via {label} on {}, {} bytes", r.itag, route.host, r.len);
                chain.worked(i);
                return Ok(r);
            }
            // The service's, not the client's: more requests only make it
            // worse.
            Answer::Failed(e @ StreamError::RateLimited { .. })
            | Answer::Player(PlayerError::Other(e @ StreamError::RateLimited { .. })) => return Err(e),
            // Refused even with fresh visitor data: the service's too.
            Answer::Player(PlayerError::Bot) => return Err(StreamError::RateLimited { retry_after: None }),
            Answer::Player(PlayerError::Other(e @ (StreamError::AgeRestricted | StreamError::RegionBlocked))) => {
                return Err(e);
            }
            Answer::Player(PlayerError::Other(e @ StreamError::Unavailable)) => {
                log::info!("youtube: {id}: unavailable to {label}");
                skip.push(route.client);
                err.get_or_insert(e);
            }
            Answer::Player(PlayerError::Other(e)) | Answer::Failed(e) => {
                chain.bench(i, &e.to_string());
                err = Some(e);
            }
            Answer::NoFormats => {
                log::info!("youtube: {id}: no usable format from {label}");
                skip.push(route.client);
                err.get_or_insert(StreamError::Unavailable);
            }
            Answer::Refused => {
                chain.bench_client(route.client, "its stream URLs are refused (403)");
                skip.push(route.client);
                err = Some(StreamError::Http { status: 403 });
            }
        }
    }
    Err(err.unwrap_or(StreamError::Unavailable))
}

/// The error and its causes, "error sending request: … : connection
/// refused", without the URL.
pub(crate) fn error_chain(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut out = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cause = c.source();
    }
    out
}

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

fn resolved(f: &Format, fallback_duration: Option<Duration>, route: Option<usize>) -> Result<Resolved, StreamError> {
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
        route,
    })
}

// --- the player answer ------------------------------------------------------

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

/// Why the player gave nothing.
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


/// One player request of `route`: the audio formats and the video length.
async fn ask_route(
    http: &reqwest::Client,
    route: Route,
    id: &str,
    gl: &str,
    visitor_data: &str,
) -> Result<(Vec<Format>, Option<Duration>), PlayerError> {
    let (client, name, version, ua) = route.client.request(gl, visitor_data);
    let body = json!({
        "context": { "client": client },
        "videoId": id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let mut req = http
        .post(format!("https://{}/youtubei/v1/player?prettyPrint=false", route.host))
        .timeout(PLAYER_TIMEOUT)
        .header(reqwest::header::USER_AGENT, ua)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-YouTube-Client-Name", name)
        .header("X-YouTube-Client-Version", version);
    if !visitor_data.is_empty() {
        req = req.header("X-Goog-Visitor-Id", visitor_data);
    }
    let network = |e: reqwest::Error| PlayerError::Other(StreamError::Network(error_chain(e)));
    let resp = req.body(body.to_string()).send().await.map_err(network)?;
    let status = resp.status().as_u16();
    if status == 429 {
        return Err(PlayerError::Other(StreamError::RateLimited {
            retry_after: None,
        }));
    }
    if status != 200 {
        return Err(PlayerError::Other(StreamError::Http { status }));
    }
    let text = resp.text().await.map_err(network)?;
    parse_player(&text)
}

/// Whether the last kilobyte of `url` answers: the URLs of some clients
/// serve only the first megabyte without a PO token.
async fn tail_answers(http: &reqwest::Client, url: &str, len: u64) -> Result<bool, StreamError> {
    let from = len.saturating_sub(1024);
    let resp = http
        .get(url)
        .timeout(TAIL_TIMEOUT)
        .header(reqwest::header::RANGE, format!("bytes={from}-{}", len.saturating_sub(1)))
        .send()
        .await
        .map_err(|e| StreamError::Network(error_chain(e)))?;
    match resp.status().as_u16() {
        200 | 206 => Ok(true),
        403 | 410 => Ok(false),
        status => Err(StreamError::Http { status }),
    }
}

/// Asks route `i` of [`ROUTES`] for `id`, with fresh visitor data once if
/// the player suspects a bot.
async fn answer(rp: &RustyPipe, http: &reqwest::Client, id: &str, gl: &str, i: usize) -> Answer {
    let route = ROUTES[i];
    let mut fresh = false;
    loop {
        let vd = match tokio::time::timeout(PLAYER_TIMEOUT, rp.query().get_visitor_data(fresh)).await {
            Ok(Ok(vd)) => vd,
            // Without: the player mostly answers all the same.
            Ok(Err(e)) => {
                log::debug!("youtube: no visitor data: {e}");
                String::new()
            }
            Err(_) => {
                log::debug!("youtube: no visitor data: timed out");
                String::new()
            }
        };
        match ask_route(http, route, id, gl, &vd).await {
            Ok((formats, duration)) => {
                let Some(f) = pick(&formats) else {
                    return Answer::NoFormats;
                };
                if route.client.check_tail() {
                    match tail_answers(http, f.url.as_deref().unwrap_or_default(), f.len).await {
                        Ok(true) => {}
                        Ok(false) => return Answer::Refused,
                        Err(e) => return Answer::Failed(e),
                    }
                }
                return match resolved(f, duration, Some(i)) {
                    Ok(r) => Answer::Stream(r),
                    Err(e) => Answer::Failed(e),
                };
            }
            Err(PlayerError::Bot) if !fresh => {
                if !vd.is_empty() {
                    rp.query().remove_visitor_data(&vd);
                }
                fresh = true;
            }
            Err(e) => return Answer::Player(e),
        }
    }
}

/// RustyPipe's player: with the user's login first when there is one (the
/// age gate opens for a signed-in adult; cookies sign in the web clients),
/// else (and unless `login_only`) its default clients.
async fn rustypipe_player(
    rp: &RustyPipe,
    id: &str,
    login_only: bool,
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
            Err(e) if login_only => return Err(e),
            Err(e) => log::info!("youtube: {id}: player with login failed: {e}"),
        }
    }
    q.player(id).await
}

/// The best stream RustyPipe's player gives, if the end of its URL
/// answers.
async fn rustypipe_stream(
    rp: &RustyPipe,
    http: &reqwest::Client,
    id: &str,
    login_only: bool,
    first_err: &StreamError,
) -> Result<Resolved, StreamError> {
    let player = rustypipe_player(rp, id, login_only).await.map_err(|e| {
        log::info!("youtube: {id}: RustyPipe player: {e}");
        // The more telling reason of the two.
        match (first_err, crate::map_err(e)) {
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
    let f = pick(&formats).ok_or_else(|| first_err.clone())?;
    if !tail_answers(http, f.url.as_deref().unwrap_or_default(), f.len).await? {
        log::info!(
            "youtube: {id}: the URL of RustyPipe's {:?} player is refused at its end",
            player.client_type
        );
        return Err(match first_err {
            StreamError::AgeRestricted => first_err.clone(),
            _ => StreamError::Http { status: 403 },
        });
    }
    log::debug!(
        "youtube: {id}: itag {} via {:?}, {} bytes",
        f.itag,
        player.client_type,
        f.len
    );
    let duration = (player.details.duration > 0)
        .then(|| Duration::from_secs(u64::from(player.details.duration)));
    resolved(f, duration, None)
}

/// The best audio stream of a video: through [`ROUTES`], then RustyPipe's
/// player, all within [`RESOLVE_BUDGET`]. `signed_in`: the user's login is
/// there (the only way past the age gate).
pub(crate) async fn resolve(
    rp: &RustyPipe,
    http: &reqwest::Client,
    chain: &Chain,
    id: &str,
    gl: &str,
    signed_in: bool,
) -> Result<Resolved, StreamError> {
    let deadline = Instant::now() + RESOLVE_BUDGET;
    let first_err = match run_chain(chain, id, deadline, |i| answer(rp, http, id, gl, i)).await {
        Ok(r) => return Ok(r),
        Err(e) => e,
    };
    let login_only = match first_err {
        StreamError::RateLimited { .. } | StreamError::RegionBlocked => return Err(first_err),
        StreamError::AgeRestricted if !signed_in => {
            log::info!("youtube: {id}: age-restricted, and no client opens it without a login");
            return Err(first_err);
        }
        StreamError::AgeRestricted => true,
        _ => false,
    };
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(first_err);
    }
    log::info!("youtube: {id}: no route gave a stream ({first_err}), trying RustyPipe's player");
    match tokio::time::timeout(left, rustypipe_stream(rp, http, id, login_only, &first_err)).await {
        Ok(r) => r,
        Err(_) => {
            log::info!("youtube: {id}: RustyPipe's player out of time");
            Err(first_err)
        }
    }
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
        let r = resolved(f, duration, Some(0)).unwrap();
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

    fn stream(route: usize) -> Resolved {
        Resolved {
            url: format!("https://x/{route}"),
            len: 1000,
            format: format::WEBM,
            duration: None,
            gain_db: None,
            itag: 251,
            bitrate: 160_000,
            route: Some(route),
        }
    }

    /// Runs the chain with canned answers by route; the routes asked, in
    /// order, and the result.
    fn run(chain: &Chain, answers: impl Fn(usize) -> Answer) -> (Vec<usize>, Result<Resolved, StreamError>) {
        run_until(chain, Instant::now() + RESOLVE_BUDGET, answers)
    }

    fn run_until(
        chain: &Chain,
        deadline: Instant,
        answers: impl Fn(usize) -> Answer,
    ) -> (Vec<usize>, Result<Resolved, StreamError>) {
        let asked = Mutex::new(Vec::new());
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let r = rt.block_on(run_chain(chain, "id", deadline, |i| {
            asked.lock().unwrap().push(i);
            std::future::ready(answers(i))
        }));
        (asked.into_inner().unwrap(), r)
    }

    fn network() -> Answer {
        Answer::Player(PlayerError::Other(StreamError::Network(
            "error sending request: connection refused".into(),
        )))
    }

    #[test]
    fn routes_start_with_visionos_and_end_with_android_vr() {
        assert_eq!(ROUTES[0], Route { client: Client::VisionOs, host: "music.youtube.com" });
        assert!(ROUTES[..3].iter().all(|r| r.client == Client::VisionOs));
        assert_eq!(ROUTES[3].client, Client::AndroidVr);
        assert!(Client::AndroidVr.check_tail() && !Client::VisionOs.check_tail());
        assert_eq!(Chain::default().order(), [0, 1, 2, 3]);
        let (c, id, version, _) = Client::VisionOs.request("RU", "vd");
        assert_eq!((c["clientName"].as_str(), id, version), (Some("VISIONOS"), "101", "1.02"));
        assert_eq!(
            (c["hl"].as_str(), c["gl"].as_str(), c["visitorData"].as_str()),
            (Some("en"), Some("RU"), Some("vd"))
        );
        let (c, _, _, _) = Client::AndroidVr.request("RU", "");
        assert!(c.get("visitorData").is_none());
    }

    #[test]
    fn a_host_that_fails_is_passed_at_once_and_the_one_that_answers_goes_first() {
        let chain = Chain::default();
        // music.youtube.com turned away (a hosts entry): the next host.
        let (asked, r) = run(&chain, |i| if i == 0 { network() } else { Answer::Stream(stream(i)) });
        assert_eq!(asked, [0, 1]);
        assert_eq!(r.unwrap().route, Some(1));
        // Next time the host that answered is asked first, the failed one
        // last.
        assert_eq!(chain.order(), [1, 2, 3, 0]);
        let (asked, _) = run(&chain, |i| Answer::Stream(stream(i)));
        assert_eq!(asked, [1]);
        // An HTTP error or a timeout of a route counts the same.
        let (asked, r) = run(&chain, |i| match i {
            1 => Answer::Failed(StreamError::Http { status: 400 }),
            2 => Answer::Failed(StreamError::Network("timed out".into())),
            _ => Answer::Stream(stream(i)),
        });
        assert_eq!(asked, [1, 2, 3]);
        assert_eq!(r.unwrap().route, Some(3));
        assert_eq!(chain.order(), [3, 0, 1, 2], "benched longest ago first");
    }

    #[test]
    fn refused_urls_bench_the_whole_client() {
        let chain = Chain::default();
        // Every visionOS host down, android_vr's URL refused at its end
        // (the 403 after the first megabyte).
        let (asked, r) = run(&chain, |i| if i < 3 { network() } else { Answer::Refused });
        assert_eq!(asked, [0, 1, 2, 3]);
        assert_eq!(r.unwrap_err(), StreamError::Http { status: 403 });
        // A visionOS URL refused while streaming: all its hosts go behind.
        let chain = Chain::default();
        chain.worked(1);
        chain.refused(&stream(1));
        assert_eq!(chain.order(), [3, 0, 1, 2]);
        // RustyPipe's URLs bench nothing.
        let mut rp = stream(0);
        rp.route = None;
        let fresh = Chain::default();
        fresh.refused(&rp);
        assert_eq!(fresh.order(), [0, 1, 2, 3]);
        // Benched routes are still asked when nothing else is left.
        let (asked, r) = run(&chain, |i| if i == 3 { Answer::Refused } else { Answer::Stream(stream(i)) });
        assert_eq!(asked, [3, 0]);
        assert_eq!(r.unwrap().route, Some(0));
        assert_eq!(chain.order()[0], 0);
    }

    #[test]
    fn the_video_refusals_stop_the_chain_or_skip_the_client() {
        let chain = Chain::default();
        let (asked, r) = run(&chain, |_| Answer::Player(PlayerError::Other(StreamError::AgeRestricted)));
        assert_eq!((asked, r.unwrap_err()), (vec![0], StreamError::AgeRestricted));
        let region = |_| Answer::Player(PlayerError::Other(StreamError::RegionBlocked));
        assert_eq!(run(&chain, region).0, [0]);
        // Unavailable to visionOS ("made for kids"): the next client, not
        // the next host; nothing benched.
        let (asked, r) = run(&chain, |i| match ROUTES[i].client {
            Client::VisionOs => Answer::Player(PlayerError::Other(StreamError::Unavailable)),
            Client::AndroidVr => Answer::Stream(stream(i)),
        });
        assert_eq!(asked, [0, 3]);
        assert_eq!(r.unwrap().route, Some(3));
        let (asked, r) = run(&Chain::default(), |_| Answer::NoFormats);
        assert_eq!((asked, r.unwrap_err()), (vec![0, 3], StreamError::Unavailable));
        // A bot check (after fresh visitor data) and a rate limit are the
        // service's: no more requests.
        let (asked, r) = run(&Chain::default(), |_| Answer::Player(PlayerError::Bot));
        assert_eq!((asked, r.unwrap_err()), (vec![0], StreamError::RateLimited { retry_after: None }));
        let limited = |_| Answer::Failed(StreamError::RateLimited { retry_after: None });
        assert_eq!(run(&Chain::default(), limited).0, [0]);
    }

    #[test]
    fn the_chain_gives_up_when_out_of_time() {
        let (asked, r) = run_until(&Chain::default(), Instant::now(), |i| Answer::Stream(stream(i)));
        assert!(asked.is_empty());
        assert_eq!(r.unwrap_err(), StreamError::Unavailable);
    }

    /// Resolves a few public videos through the chain, anonymously:
    /// `cargo test -p nikgob-youtube --lib live_chain -- --ignored --nocapture`.
    #[test]
    #[ignore = "network"]
    fn live_chain() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Info)
            .try_init();
        let dir = tempfile::tempdir().unwrap();
        let rp = RustyPipe::builder()
            .storage_dir(dir.path())
            .no_botguard()
            .build()
            .unwrap();
        let http = reqwest::Client::new();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let chain = Chain::default();
        for id in ["cGmkwjw5oF0", "kPkft7RLX9s", "IN28bAkcqQk", "x3Yl4a5Eaj4", "feFFauF1GWk", "aZY3gguR2uM"] {
            let started = Instant::now();
            let r = rt.block_on(resolve(&rp, &http, &chain, id, "RU", false)).unwrap();
            let route = ROUTES[r.route.expect("one of ours")];
            let tail = rt.block_on(tail_answers(&http, &r.url, r.len));
            println!(
                "{id}: itag {} {} bytes via {} on {} in {} ms, the end answers: {tail:?}",
                r.itag,
                r.len,
                route.client.label(),
                route.host,
                started.elapsed().as_millis()
            );
            assert_ne!(tail, Ok(false), "{id}: the whole file is served");
        }
        // Every route, asked directly.
        for (i, route) in ROUTES.iter().enumerate() {
            let a = rt.block_on(super::answer(&rp, &http, "cGmkwjw5oF0", "RU", i));
            let said = match a {
                Answer::Stream(r) => format!("a stream, itag {}", r.itag),
                other => format!("{other:?}"),
            };
            println!("{} on {}: {said}", route.client.label(), route.host);
        }
        let started = Instant::now();
        let e = rt
            .block_on(resolve(&rp, &http, &chain, "tRAmPLYjbGg", "RU", false))
            .unwrap_err();
        println!("tRAmPLYjbGg: {e} in {} ms", started.elapsed().as_millis());
        assert_eq!(e, StreamError::AgeRestricted);
        assert!(started.elapsed() < RESOLVE_BUDGET);
    }
}
