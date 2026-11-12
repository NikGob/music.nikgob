//! Playback resolution beyond phase 1's source order: a track that only has
//! sources nobody can play right now (Spotify without its sound for this
//! account, a file that is gone) is looked up on YouTube Music in the
//! background — ISRC first, then "artist title" with the official song
//! preferred over a clip over an upload, a length within 3 s and the best
//! title match — and the hit is attached to the same track id as one more
//! source. A hit must be the same song of the same artist (any spelling
//! of its name): never a cover, a karaoke, a remix or another performer's
//! version (docs/stage4.5.md, «Известные проблемы стриминга YouTube»); no
//! such hit is an honest «не нашёл» with «Найти песню…», not a cover. Misses are not asked again for days (kept in the library's
//! cache, so a restart does not ask either). The lookups ask one category
//! at a time: songs, videos only when no song fits (2 requests instead of
//! up to 11). The next one or two tracks of the queue are looked up ahead,
//! one after the other, so a Spotify-only track starts without waiting.
//!
//! A Deezer track (the catalogue has no sound) goes the same way. Deezer
//! lists album tracks without their ISRC, so a track that has none yet is
//! first asked of Deezer alone (`/track/{id}`, one request): the ISRC it
//! brings is kept on the track and makes the YouTube lookup exact.
//!
//! While YouTube Music is off (or gives no sound) and Spotify plays, the
//! track being played and the ones ahead are looked up on Spotify the same
//! way instead (ISRC, then "artist title", songs only), and the hit is
//! attached as a Spotify source. A track that starts from YouTube Music
//! while Spotify plays is looked up on Spotify too, in the background: the
//! play goes on, the next one is from Spotify. The stream analysis and
//! saving from YouTube Music only ever ask YouTube Music.
//!
//! A rate limit is not a miss: nothing is recorded, the lookups ahead wait
//! for the pause to end, and a track that waits for its lookup (Buffering)
//! is looked up again by itself when a pause of up to a minute ends («YouTube
//! Music просит подождать — продолжу сам»), at most three times. After a
//! longer pause playback stops and the toast says when to try again.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::{SearchKind, Service, StreamTrack, TrackKind};
use nikgob_metadata::text::{artist_key, search_artist, search_title, similarity};

use super::Msg;
use crate::Worker;
use crate::queue::Entry;

/// Lengths this far apart (ms) are another recording.
const SLACK_MS: u64 = 3_000;
/// A hit needs at least this title similarity (0..=1).
const MIN_TITLE: f64 = 0.7;
/// A hit needs this artist similarity (0..=1).
const MIN_ARTIST: f64 = 0.8;

/// What is looked up on YouTube Music for a track.
#[derive(Clone, Debug)]
pub(crate) struct ResolveQuery {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) isrc: Option<String>,
    /// Videos that did not play (age gate, region block): another
    /// candidate is picked instead.
    pub(crate) exclude: Vec<String>,
    /// Other names of the artist (its aliases, the credit's other names).
    pub(crate) aliases: Vec<String>,
}

impl ResolveQuery {
    /// The search text: "artist title" without release noise.
    pub(crate) fn text(&self) -> String {
        format!("{} {}", search_artist(&self.artist), search_title(&self.title))
    }
}

fn kind_rank(k: TrackKind) -> u8 {
    match k {
        TrackKind::Song => 0,
        TrackKind::Video => 1,
        TrackKind::Upload => 2,
    }
}

/// Title similarity of a hit to the wished title, over the forms each goes
/// by (a title in two scripts, "火星人 - Martian", matches either); a clip
/// titled "Artist - Title (…)" is also compared without its "Artist - ".
fn title_score(hit: &StreamTrack, want: &[String]) -> f64 {
    let mut got = nikgob_sources::text::title_keys(&hit.title);
    if let Some((_, rest)) = hit.title.split_once(" - ") {
        got.extend(nikgob_sources::text::title_keys(rest));
    }
    got.iter()
        .flat_map(|g| want.iter().map(move |w| similarity(g, w)))
        .fold(0.0, f64::max)
}

/// How much a hit is by the wished artist (0..=1): a credited artist alike
/// any of its names, or a clip titled "Artist - Title" by an uploader.
fn artist_score(hit: &StreamTrack, q: &ResolveQuery) -> f64 {
    let names: Vec<&str> = std::iter::once(q.artist.as_str())
        .chain(q.artist.split([',', '&']).map(str::trim))
        .chain(q.aliases.iter().map(String::as_str))
        .filter(|n| !n.is_empty())
        .collect();
    let keys: Vec<String> = names.iter().map(|n| artist_key(n)).filter(|k| !k.is_empty()).collect();
    let alike = |name: &str| -> f64 {
        if names.iter().any(|n| nikgob_core::names::same_name(n, name)) {
            return 1.0;
        }
        let k = artist_key(name);
        keys.iter().map(|w| similarity(&k, w)).fold(0.0, f64::max)
    };
    let credited = hit.artists.iter().map(|a| alike(&a.name)).fold(0.0, f64::max);
    let in_title = hit.title.split_once(" - ").map_or(0.0, |(head, _)| alike(head.trim()));
    credited.max(in_title)
}

/// A hit is another version of the song (a cover, a karaoke, a remix, a
/// sped-up upload) that the wish does not name.
fn other_version(hit: &StreamTrack, q: &ResolveQuery) -> bool {
    nikgob_sources::matching::other_version(&hit.title, &q.title).is_some()
}

/// The best YouTube hit for `q`, if any is the song: playable, not refused,
/// the length within 3 s, the title alike, by the artist, not another
/// version.
pub(crate) fn pick_youtube(hits: &[StreamTrack], q: &ResolveQuery) -> Option<StreamTrack> {
    let want = nikgob_sources::text::title_keys(&q.title);
    let mut ranked: Vec<(u8, f64, &StreamTrack)> = hits
        .iter()
        .filter(|h| h.playable && !q.exclude.contains(&h.id.id))
        .filter(|h| match (h.duration_ms, q.duration_ms) {
            (Some(a), Some(b)) => a.abs_diff(b) <= SLACK_MS,
            _ => true,
        })
        .filter(|h| !other_version(h, q))
        .filter_map(|h| {
            let title = title_score(h, &want);
            if title < MIN_TITLE {
                return None;
            }
            let artist = artist_score(h, q);
            if artist < MIN_ARTIST {
                return None;
            }
            Some((kind_rank(h.kind), title + 0.3 * artist, h))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    ranked.first().map(|(.., h)| (*h).clone())
}

/// The hit of an ISRC lookup: the recording itself, whoever YouTube
/// credits it to (an artist in another script), if its length fits and it
/// is not another version.
fn pick_isrc(hit: &StreamTrack, q: &ResolveQuery) -> bool {
    hit.playable
        && !q.exclude.contains(&hit.id.id)
        && !other_version(hit, q)
        && match (hit.duration_ms, q.duration_ms) {
            (Some(a), Some(b)) => a.abs_diff(b) <= SLACK_MS,
            _ => title_score(hit, &nikgob_sources::text::title_keys(&q.title)) >= MIN_TITLE,
        }
}

/// Tracks to look up ahead in the queue.
const AHEAD: usize = 2;
/// Library cache key of a lookup on `via` that found nothing.
fn miss_key(track: TrackId, via: Service) -> String {
    match via {
        Service::Spotify => format!("sp_miss.{track}"),
        _ => format!("yt_miss.{track}"),
    }
}

/// Re-lookups of a waiting track after rate limits.
const LIMIT_RETRIES: u32 = 3;
/// The pause assumed when YouTube Music does not say how long it is.
const UNKNOWN_PAUSE: Duration = Duration::from_secs(30);
/// How long a miss keeps a track the user plays from being looked up again.
const PLAYED_MISS_TTL: Duration = Duration::from_secs(10 * 60);

/// A lookup's answer: the hit (or none), or `Err(retry_after)` when the
/// service rate-limited it.
pub(crate) type Lookup = Result<Option<StreamTrack>, Option<Duration>>;

/// Rate-limit state of the lookups.
#[derive(Default)]
pub(crate) struct ResolveLimit {
    /// No lookup ahead starts before this.
    until: Option<Instant>,
    /// Re-lookups of the waiting track so far.
    retries: HashMap<TrackId, u32>,
}

/// Songs, then videos only when no song fits (YouTube Music: Spotify
/// has songs only); a rate limit stops it.
async fn songs_then_videos(
    yt: &dyn nikgob_core::streaming::StreamService,
    text: &str,
    keep: &(dyn Fn(&StreamTrack) -> bool + Sync),
    q: &ResolveQuery,
) -> Lookup {
    let who = yt.service().display_name();
    let kinds: &[SearchKind] = if yt.service() == Service::Youtube {
        &[SearchKind::Songs, SearchKind::Videos]
    } else {
        &[SearchKind::Songs]
    };
    for &kind in kinds {
        match yt.search_kind(text, kind).await {
            Ok(r) => {
                let hits: Vec<StreamTrack> = r.tracks.into_iter().filter(|t| keep(t)).collect();
                if let Some(hit) = pick_youtube(&hits, q) {
                    return Ok(Some(hit));
                }
            }
            Err(e @ nikgob_core::streaming::StreamError::RateLimited { retry_after }) => {
                log::warn!("{who} lookup of {text}: {e}");
                return Err(retry_after);
            }
            Err(e) => log::warn!("{who} lookup of {text}: {e}"),
        }
    }
    Ok(None)
}

/// "artist title" first; when that finds nothing, the title alone (the
/// artist is checked on every hit). A rate limit is `None` too.
pub(crate) async fn search_youtube(
    yt: &dyn nikgob_core::streaming::StreamService,
    q: &ResolveQuery,
) -> Option<StreamTrack> {
    lookup_youtube(yt, q).await.ok().flatten()
}

/// [`search_youtube`] telling a rate limit apart.
async fn lookup_youtube(yt: &dyn nikgob_core::streaming::StreamService, q: &ResolveQuery) -> Lookup {
    if let Some(hit) = songs_then_videos(yt, &q.text(), &|_| true, q).await? {
        return Ok(Some(hit));
    }
    songs_then_videos(yt, &search_title(&q.title), &|_| true, q).await
}

/// Why a stream did not play, from the engine's error text, as the user
/// reads it ("ARISTOCRAT: <this>"), and the settings row that helps, if
/// one does (the toast's «Войти»).
pub(crate) fn failure_text(
    message: &str,
    youtube_signed_in: bool,
    service: Option<Service>,
) -> (String, Option<nikgob_view::SettingLink>) {
    use nikgob_view::SettingLink;
    let m = message.to_lowercase();
    if m.contains("age-restricted") {
        if youtube_signed_in {
            ("на YouTube только для 18+, и со входом не открылся".into(), None)
        } else {
            ("на YouTube только для 18+ — нужен вход в YouTube Music".into(), Some(SettingLink::Youtube))
        }
    } else if m.contains("drm") {
        ("в SoundCloud только в защищённом виде".into(), None)
    } else if m.contains("blocked in this region") {
        ("недоступно в твоём регионе".into(), None)
    } else if m.contains("premium") {
        ("для звука из Spotify нужен Premium".into(), None)
    } else if m.contains("rate limited") {
        // Playback goes on with the next track; this one waits for the user.
        let who = service.map_or("сервис", |s| s.display_name());
        (format!("{who} просит подождать — включи трек чуть позже"), None)
    } else if m.contains("not available") || m.contains("недоступ") {
        ("трек недоступен".into(), None)
    } else if service == Some(Service::Youtube) && (m.contains("refused") || m.contains("http 403")) {
        // Every client of the player was asked (see nikgob_youtube::player):
        // YouTube's, not the video's.
        ("YouTube сейчас не отдаёт звук — попробуй чуть позже".into(), None)
    } else if service == Some(Service::Youtube) && (m.contains("network") || m.contains("stalled")) {
        ("нет связи с YouTube — проверь интернет".into(), None)
    } else {
        ("не получилось воспроизвести".into(), None)
    }
}

/// Whether a stream's error text says the video itself does not play
/// (another copy of the song may).
pub(crate) fn video_refused(message: &str) -> bool {
    let m = message.to_lowercase();
    ["age-restricted", "blocked in this region", "not available"]
        .iter()
        .any(|k| m.contains(k))
}

impl Worker {
    /// Videos refused lately (see [`Self::stream_failed`]).
    pub(crate) fn refused_videos(&mut self) -> Vec<String> {
        self.streaming
            .refused
            .retain(|_, at| at.elapsed() < super::REFUSED_TTL);
        self.streaming.refused.keys().cloned().collect()
    }

    /// The playing source of `track` did not open: a YouTube video that
    /// is refused (age gate, region block, gone) is set aside, so the next
    /// lookup picks another copy of the song; the reason is kept for the
    /// toast if none plays. Whether the failure was the video's: anything
    /// else of YouTube (stream URLs refused by every client of the player,
    /// no connection) fails the other copies the same way, so they are not
    /// tried.
    pub(crate) fn stream_failed(&mut self, track: TrackId, source: nikgob_core::model::SourceId, message: &str) -> bool {
        let Some(src) = self.lib.get_source(source).ok().flatten() else { return true };
        let signed_in = self
            .service(Service::Youtube)
            .is_some_and(|y| y.account().is_logged_in());
        let why = failure_text(message, signed_in, src.kind.service());
        log::info!("{track}: {} source {} failed: {message}", src.kind.as_str(), src.locator);
        self.streaming.failures.insert(track, why);
        if src.kind == SourceKind::Youtube && !video_refused(message) {
            return false;
        }
        if src.kind == SourceKind::Youtube {
            self.streaming.refused.insert(src.locator.clone(), Instant::now());
            if let Err(e) = self.lib.set_source_available(source, false) {
                log::warn!("setting aside the YouTube copy of {track}: {e}");
            }
        }
        true
    }

    /// The track can be looked up on YouTube Music: YouTube is on, the
    /// track has no YouTube source yet, and it did not miss recently.
    pub(crate) fn can_resolve(&mut self, track: TrackId) -> bool {
        let ttl = self.streaming.schedule.miss_ttl;
        self.can_resolve_within(track, ttl)
    }

    /// [`Self::can_resolve`] with a miss older than `ttl` forgotten.
    fn can_resolve_within(&mut self, track: TrackId, ttl: Duration) -> bool {
        self.can_resolve_on(track, ttl, Service::Youtube)
    }

    /// Where a track that plays from nothing now is looked up: YouTube
    /// Music, or Spotify while YouTube Music is off or silent (see the
    /// module docs). `None` when neither can be asked.
    fn play_lookup(&mut self, track: TrackId, ttl: Duration) -> Option<Service> {
        if self.streaming.resolving.contains(&track) {
            return Some(self.streaming.resolve_via.get(&track).copied().unwrap_or(Service::Youtube));
        }
        if self.service(Service::Youtube).is_some() && self.has_audio(Service::Youtube) {
            return self.can_resolve_on(track, ttl, Service::Youtube).then_some(Service::Youtube);
        }
        self.can_resolve_on(track, ttl, Service::Spotify).then_some(Service::Spotify)
    }

    /// The track can be looked up on `via`: it is on and plays, the track
    /// has no copy there yet, and it did not miss there within `ttl`.
    fn can_resolve_on(&mut self, track: TrackId, ttl: Duration, via: Service) -> bool {
        if self.service(via).is_none() || !self.has_audio(via) {
            return false;
        }
        if self.streaming.resolving.contains(&track) {
            return true;
        }
        let ttl = ttl.as_millis() as i64;
        let now = nikgob_library::now_ms();
        let missed = match self.streaming.misses.get(&(track, via)) {
            Some(at) => Some(*at),
            None => self
                .lib
                .cache_get(&miss_key(track, via))
                .ok()
                .flatten()
                .map(|(at, _)| at),
        };
        if let Some(at) = missed {
            if now - at < ttl {
                self.streaming.misses.insert((track, via), at);
                return false;
            }
            self.streaming.misses.remove(&(track, via));
        }
        let kind = if via == Service::Spotify { SourceKind::Spotify } else { SourceKind::Youtube };
        let has_copy = self.lib.sources_of_track(track).unwrap_or_default().iter().any(|s| {
            s.kind == kind && s.available && s.trashed_at.is_none()
        });
        !has_copy && self.lib.get_track(track).ok().flatten().is_some()
    }

    /// Every name of the track's artists the library knows: the credited
    /// names, the artists' own names and their aliases.
    pub(crate) fn artist_names(&self, t: &nikgob_core::model::Track) -> Vec<String> {
        let ids: Vec<nikgob_core::model::ArtistId> = t.artist_credit.0.iter().filter_map(|p| p.artist_id).collect();
        let mut out: Vec<String> = t.artist_credit.0.iter().map(|p| p.name.clone()).collect();
        for id in &ids {
            if let Some(a) = self.lib.get_artist(*id).ok().flatten() {
                out.push(a.name);
            }
        }
        if !ids.is_empty() {
            for (who, name) in self.lib.artist_aliases().unwrap_or_default() {
                if ids.contains(&who) {
                    out.push(name);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The Deezer id of a track that has no ISRC yet, and the catalogue to
    /// ask it of (see the module docs).
    pub(crate) fn isrc_source(&self, track: TrackId) -> Option<(std::sync::Arc<dyn nikgob_core::streaming::StreamService>, String)> {
        let dz = self.service(Service::Deezer)?;
        let id = self
            .lib
            .external_ids(nikgob_core::streaming::EntityKind::Track, *track.as_uuid())
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.service == Service::Deezer)?
            .id;
        Some((dz, id))
    }

    /// Starts the YouTube lookup of `track` (once).
    pub(crate) fn begin_resolve(&mut self, track: TrackId) {
        self.begin_resolve_on(track, Service::Youtube);
    }

    /// Starts the lookup of `track` on `via` (once).
    fn begin_resolve_on(&mut self, track: TrackId, via: Service) {
        if self.streaming.resolving.contains(&track) {
            return;
        }
        let Some(yt) = self.service(via) else { return };
        let Some(t) = self.lib.get_track(track).ok().flatten() else { return };
        let mut q = ResolveQuery {
            title: t.title.clone(),
            artist: t.artist_credit.display(),
            duration_ms: t.duration_ms.map(u64::from),
            isrc: t.isrc.clone(),
            exclude: self.refused_videos(),
            aliases: self.artist_names(&t),
        };
        let deezer = if q.isrc.is_none() { self.isrc_source(track) } else { None };
        self.streaming.resolving.insert(track);
        self.streaming.resolve_via.insert(track, via);
        log::info!("looking up {} on {}", q.text(), via.display_name());
        self.streaming.spawn(async move {
            // A Deezer track without its ISRC: Deezer tells it first.
            let mut filled = None;
            if let Some((dz, id)) = deezer {
                match dz.track(&id).await {
                    Ok(full) => {
                        q.isrc = full.isrc.clone();
                        filled = Some(Box::new(full));
                    }
                    Err(e) => log::info!("deezer: no ISRC for {id}: {e}"),
                }
            }
            if let Some(isrc) = &q.isrc
                && let Ok(Some(hit)) = yt.find_isrc(isrc).await
                && pick_isrc(&hit, &q)
            {
                return Msg::Resolved {
                    track,
                    via,
                    found: Ok(Some(hit)),
                    filled,
                };
            }
            Msg::Resolved {
                track,
                via,
                found: lookup_youtube(yt.as_ref(), &q).await,
                filled,
            }
        });
    }

    /// The full Deezer track of a library track came (with its ISRC): the
    /// ISRC fills the track's empty one.
    pub(crate) fn isrc_filled(&mut self, track: TrackId, full: &StreamTrack) {
        if full.isrc.is_none() {
            return;
        }
        self.streaming.learn(std::slice::from_ref(full));
        if let Err(e) = self.lib.attach_stream_source(track, full) {
            log::warn!("keeping the ISRC of {track}: {e}");
        }
    }

    /// `e` has nothing playable now but can be looked up: it waits
    /// (Buffering) for the lookup instead of being skipped.
    pub(crate) fn wait_for_youtube(&mut self, e: Entry) -> bool {
        // A track being played is looked up again unless it was missed just
        // now: a miss of a lookup ahead (or of a search that failed) must
        // not keep it silent for days.
        let Some(via) = self.play_lookup(e.track, PLAYED_MISS_TTL) else {
            return false;
        };
        self.begin_resolve_on(e.track, via);
        self.playback.stop();
        self.playing = Some(e);
        self.playing_source = None;
        self.restarted = None;
        self.status = nikgob_view::Status::Buffering;
        // A copy that failed mid-track: the one found goes on from there.
        self.position = self.failover.filter(|(id, _)| *id == e.id).map_or(std::time::Duration::ZERO, |(_, at)| at);
        self.position_at = Instant::now();
        self.rate = 1.0;
        self.publish_now_playing();
        true
    }

    /// [`nikgob_view::Command::PlayFromYoutube`]: the playing track goes on
    /// from where it is on its YouTube Music copy, its other sources set
    /// aside for this play. Without a copy one is looked up (and attached,
    /// so the next time it is there at once); meanwhile it waits as a
    /// stream does. Nothing changes when YouTube Music cannot play now or
    /// missed the song a moment ago.
    pub(crate) fn play_from_youtube(&mut self) {
        let Some(e) = self.playing else { return };
        if self.playing_source.is_some_and(|(_, kind)| kind == SourceKind::Youtube) {
            return;
        }
        let sources = self.lib.sources_of_track(e.track).unwrap_or_default();
        let copy = self.has_audio(Service::Youtube)
            && sources.iter().any(|s| s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none());
        if !copy && !self.can_resolve_on(e.track, PLAYED_MISS_TTL, Service::Youtube) {
            log::info!("{}: no YouTube Music copy to play now", e.track);
            self.toast("Нет на YouTube Music", None);
            return;
        }
        log::info!("{}: from YouTube Music at {} s", e.track, self.position_now().as_secs());
        self.failover = Some((e.id, self.position_now()));
        self.tried.extend(sources.iter().filter(|s| s.kind != SourceKind::Youtube).map(|s| s.id));
        self.try_next_source(e);
    }

    pub(crate) fn resolved(&mut self, track: TrackId, via: Service, found: Lookup, filled: Option<Box<StreamTrack>>) {
        self.streaming.resolving.remove(&track);
        self.streaming.resolve_via.remove(&track);
        if via == Service::Youtube {
            self.analysis_resolved(track, &found);
        }
        if let Some(full) = &filled {
            self.isrc_filled(track, full);
        }
        let found = match found {
            Ok(f) => {
                self.streaming.resolve_limit.retries.remove(&track);
                f
            }
            Err(retry_after) => return self.resolve_limited(track, via, retry_after),
        };
        match &found {
            Some(hit) => {
                log::info!("{track} plays from {} as {}", via.display_name(), hit.id.id);
                self.streaming.learn(std::slice::from_ref(hit));
                if let Err(e) = self.lib.attach_stream_source(track, hit) {
                    log::warn!("attaching the {} copy of {track}: {e}", via.display_name());
                }
            }
            None => {
                log::info!("{track}: no copy on {}", via.display_name());
                let now = nikgob_library::now_ms();
                self.streaming.misses.insert((track, via), now);
                let _ = self.lib.cache_put(&miss_key(track, via), now, "");
            }
        }
        let why = self.streaming.failures.remove(&track);
        if via == Service::Youtube
            && let Some(item) = self.streaming.save_after_resolve.remove(&track)
        {
            self.wish_save_from_youtube(item);
        }
        let waiting = self
            .playing
            .filter(|e| e.track == track && self.playing_source.is_none());
        if let Some(e) = waiting {
            let paused = self.status == nikgob_view::Status::Paused;
            // Not the sources that failed a moment ago (Spotify refusing
            // sound): straight to the copy just found.
            let skip = self.tried.clone();
            match self.request_skipping(e, &skip) {
                Some((req, source, kind)) => {
                    self.start_with(e, req, source, kind);
                    self.seek_to_failover(e);
                    if paused {
                        self.playback.pause();
                        self.status = nikgob_view::Status::Paused;
                        self.publish_player();
                    }
                }
                None => {
                    let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
                    // Better an honest «не нашёл» than someone else's
                    // version: the song can be picked by hand.
                    let find = Some(("Найти песню…".to_owned(), nikgob_view::Command::FindSong { track, query: None }));
                    match why {
                        // The only copies did not play: say why.
                        Some((why, Some(link))) => {
                            self.toast_open(format!("{title}: {why}"), "Войти", nikgob_view::Link::Setting(link));
                        }
                        Some((why, None)) => self.toast(format!("{title}: {why}"), find),
                        None => self.toast(format!("{title}: не нашёл эту песню на {}", via.display_name()), find),
                    }
                    match self.queue.skip_forward() {
                        Some(n) if n.id != e.id => self.start(n),
                        _ => self.stop(),
                    }
                }
            }
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track) {
            self.preload_next();
        }
        self.resolve_ahead();
    }

    /// YouTube Music rate-limited the lookup of `track` (see the module
    /// docs): not a miss. The waiting track is looked up again when a short
    /// pause ends; after a long one playback stops and says until when.
    fn resolve_limited(&mut self, track: TrackId, via: Service, retry_after: Option<Duration>) {
        let who = via.display_name();
        let pause = retry_after.or_else(|| self.pause_left(via));
        let wait = pause.unwrap_or(UNKNOWN_PAUSE);
        self.streaming.resolve_limit.until = Some(Instant::now() + wait);
        self.streaming.failures.remove(&track);
        let waiting = self
            .playing
            .filter(|e| e.track == track && self.playing_source.is_none());
        if waiting.is_none() {
            // A lookup ahead: done again when the pause is over.
            log::info!("{track}: {who} lookup rate limited, later");
            return;
        }
        let n = self.streaming.resolve_limit.retries.entry(track).or_default();
        *n += 1;
        let n = *n;
        if wait <= super::search::SHORT_WAIT && n <= LIMIT_RETRIES {
            log::info!("{track}: {who} lookup rate limited, again in {} s ({n} of {LIMIT_RETRIES})", wait.as_secs());
            if n == 1 {
                self.toast(format!("{who} просит подождать — продолжу сам"), None);
            }
            let after = wait + super::schedule::wake_jitter();
            self.streaming.spawn(async move {
                tokio::time::sleep(after).await;
                Msg::ResolveRetry(track)
            });
            return;
        }
        self.streaming.resolve_limit.retries.remove(&track);
        let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
        let until = pause
            .filter(|p| *p > super::search::SHORT_WAIT)
            .and_then(super::accounts::hm_after)
            .map(|hm| format!(" до {hm}"))
            .unwrap_or_default();
        log::info!("{track}: {who} lookup still rate limited: stopping");
        // Not skipped: the next tracks would hit the same pause.
        self.toast(format!("{title}: {who} просит подождать{until} — включи трек позже"), None);
        self.stop();
    }

    /// The pause of a waiting track's lookup ended: it is looked up again
    /// if it still waits.
    pub(crate) fn resolve_retry(&mut self, track: TrackId) {
        let waiting = self
            .playing
            .is_some_and(|e| e.track == track && self.playing_source.is_none());
        if !waiting {
            self.streaming.resolve_limit.retries.remove(&track);
            return;
        }
        if let Some(via) = self.play_lookup(track, Duration::ZERO) {
            self.begin_resolve_on(track, via);
        }
    }

    /// A track started from YouTube Music while Spotify plays: when it has
    /// no Spotify copy yet (and did not miss there lately), Spotify is
    /// asked in the background (ISRC first), and the hit joins the same
    /// track. This play goes on; the next one is from Spotify.
    pub(crate) fn spotify_copy_for(&mut self, e: Entry, kind: SourceKind) {
        if kind != SourceKind::Youtube {
            return;
        }
        let ttl = self.streaming.schedule.miss_ttl;
        if self.can_resolve_on(e.track, ttl, Service::Spotify) {
            self.begin_resolve_on(e.track, Service::Spotify);
        }
    }

    /// Looks up the next [`AHEAD`] tracks of the queue that only YouTube
    /// Music may have, one at a time (the next starts when this one
    /// answered), whatever the gapless setting; none while YouTube Music
    /// asked to wait.
    pub(crate) fn resolve_ahead(&mut self) {
        // Clips the app queued: their songs first (see `clips::QueueClips`).
        self.clips_ahead();
        if !self.streaming.resolving.is_empty() {
            return; // one lookup at a time; the playing track's goes first
        }
        if self.streaming.resolve_limit.until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let mut next: Vec<Entry> = self.queue.peek_auto_next().into_iter().collect();
        next.extend(self.queue.upcoming().iter().skip(1).take(AHEAD - 1).copied());
        let current = self.queue.current().map(|c| c.track);
        for e in next {
            if Some(e.track) == current {
                continue;
            }
            // Playable as it is (its source only: no file is read here).
            if self.pick_source(e, &[]).is_some() {
                continue;
            }
            let ttl = self.streaming.schedule.miss_ttl;
            if let Some(via) = self.play_lookup(e.track, ttl) {
                self.begin_resolve_on(e.track, via);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{ArtistRef, RemoteId};

    fn hit(id: &str, title: &str, secs: u64, kind: TrackKind) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![ArtistRef::new(None, "Кино")];
        t.duration_ms = Some(secs * 1000);
        t.kind = kind;
        t
    }

    #[test]
    fn songs_first_then_clips_with_the_same_length() {
        let q = ResolveQuery {
            title: "Группа крови".into(),
            artist: "Кино".into(),
            duration_ms: Some(285_000),
            isrc: None,
            exclude: Vec::new(),
            aliases: Vec::new(),
        };
        assert_eq!(q.text(), "Кино Группа крови");
        let clip = hit("clip", "Кино - Группа крови (клип)", 286, TrackKind::Video);
        let song = hit("song", "Группа крови", 284, TrackKind::Song);
        let long = hit("long", "Группа крови", 300, TrackKind::Song);
        let other = hit("other", "Звезда по имени Солнце", 285, TrackKind::Song);
        let upload = hit("up", "Группа крови", 285, TrackKind::Upload);
        let all = [clip.clone(), long.clone(), other.clone(), upload, song.clone()];
        assert_eq!(pick_youtube(&all, &q).unwrap().id.id, "song");
        assert_eq!(pick_youtube(&[clip.clone(), long, other], &q).unwrap().id.id, "clip");
        assert!(pick_youtube(&[hit("x", "Война", 285, TrackKind::Song)], &q).is_none());
        let mut gone = song;
        gone.playable = false;
        assert!(pick_youtube(&[gone], &q).is_none());
        // A video that did not play: the next candidate.
        let q = ResolveQuery {
            exclude: vec!["song".into()],
            ..q
        };
        assert_eq!(pick_youtube(&all, &q).unwrap().id.id, "clip");
    }

    #[test]
    fn covers_and_other_performers_are_never_picked() {
        // The live log: after a 403 the lookup played guitar covers and
        // another singer's version of a ZUTOMAYO song.
        let q = ResolveQuery {
            title: "勘冴えて悔しいわ - Kan Saete Kuyashiiwa".into(),
            artist: "ZUTOMAYO".into(),
            duration_ms: Some(228_000),
            isrc: None,
            exclude: Vec::new(),
            aliases: vec!["ずっと真夜中でいいのに。".into()],
        };
        let by = |id: &str, title: &str, artist: &str, secs: u64, kind: TrackKind| {
            let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
            t.artists = vec![ArtistRef::new(None, artist)];
            t.duration_ms = Some(secs * 1000);
            t.kind = kind;
            t
        };
        let guitar = by("g", "勘冴えて悔しいわ - Kan Saete Kuyashiiwa (Guitar Cover)", "Some Guitarist", 228, TrackKind::Video);
        let singer = by("s", "勘冴えて悔しいわ - Kan Saete Kuyashiiwa", "Another Singer", 228, TrackKind::Video);
        let karaoke = by("k", "勘冴えて悔しいわ (Karaoke)", "ZUTOMAYO", 228, TrackKind::Video);
        let mine = by("z", "勘冴えて悔しいわ", "ずっと真夜中でいいのに。", 229, TrackKind::Song);
        assert!(pick_youtube(&[guitar.clone(), singer.clone(), karaoke.clone()], &q).is_none(), "an honest miss");
        assert_eq!(pick_youtube(&[guitar, singer, karaoke, mine], &q).unwrap().id.id, "z", "the artist under its other name");
        // An uploader's clip titled "Artist - Title" is the artist's.
        let clip = by("c", "ZUTOMAYO - Kan Saete Kuyashiiwa", "ZUTOMAYO Topic Fan", 228, TrackKind::Video);
        assert_eq!(pick_youtube(&[clip], &q).unwrap().id.id, "c");
        // The ISRC's hit is the recording, credited in any script.
        let isrc = by("i", "Kan Saete Kuyashiiwa", "ZTMY", 228, TrackKind::Song);
        assert!(pick_isrc(&isrc, &q));
        assert!(!pick_isrc(&by("i2", "Kan Saete Kuyashiiwa (Remix)", "ZTMY", 228, TrackKind::Song), &q));
    }

    #[test]
    fn youtube_failures_say_whose_they_are() {
        let yt = Some(Service::Youtube);
        let refused = "i/o error: stream URL keeps being refused (HTTP 403)";
        assert!(!video_refused(refused));
        assert_eq!(failure_text(refused, false, yt).0, "YouTube сейчас не отдаёт звук — попробуй чуть позже");
        let offline = "network error: error sending request: dns error";
        assert!(!video_refused(offline));
        assert_eq!(failure_text(offline, false, yt).0, "нет связи с YouTube — проверь интернет");
        let age = "i/o error: age-restricted";
        assert!(video_refused(age));
        assert_eq!(failure_text(age, false, yt).1, Some(nikgob_view::SettingLink::Youtube));
        assert!(video_refused("not available") && video_refused("blocked in this region"));
        assert_eq!(failure_text("whatever", false, None).0, "не получилось воспроизвести");
    }
}
