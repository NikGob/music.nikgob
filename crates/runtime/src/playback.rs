use super::*;

use nikgob_core::model::{EndReason, EventMeta, EventOrigin, EventType, PlaylistId, SourceId, SourceKind};
use nikgob_core::streaming::Service as StreamKind;
use nikgob_view::LIKED_PLAYLIST;

/// Up to this far apart, our position and the heard one are the same.
const POSITION_JITTER: Duration = Duration::from_millis(4);
/// Further apart than this, our position jumps to the heard one (a seek, a
/// loop, a long stall); closer, it runs faster or slower until they meet.
const POSITION_SNAP: Duration = Duration::from_millis(300);
/// The pace is off by the difference over this (half of it is gone by
/// the next report, a quarter second later) ...
const SLEW_SECS: f64 = 0.5;
/// ... with the pace at most this much off the normal one.
pub(super) const SLEW_MAX: f64 = 0.25;
/// What to do with our position when the engine reports the heard one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Follow {
    Keep,
    /// Go on from where we are at this pace.
    Rate(f64),
    /// Jump to the heard position.
    Snap,
}

/// How our position `ours` follows the heard one (`stalled`: it has not
/// moved since the last report) while playing at `rate`. It never moves
/// backwards: ahead of a stalled sound it waits, ahead of a moving one it
/// slows down.
#[cfg(test)]
pub(super) fn follow(ours: Duration, heard: Duration, stalled: bool, rate: f64) -> Follow {
    follow_at(ours, heard, stalled, rate, 1.0)
}

/// [`follow`] for a track whose position moves `speed` seconds a second
/// (a track played faster or slower: positions are its own time).
fn follow_at(ours: Duration, heard: Duration, stalled: bool, rate: f64, speed: f64) -> Follow {
    if ours.abs_diff(heard) > POSITION_SNAP {
        return Follow::Snap;
    }
    if stalled && ours >= heard {
        return Follow::Rate(0.0);
    }
    if ours.abs_diff(heard) <= POSITION_JITTER {
        return if rate == speed { Follow::Keep } else { Follow::Rate(speed) };
    }
    let error = heard.as_secs_f64() - ours.as_secs_f64();
    Follow::Rate((speed + error / SLEW_SECS).clamp(speed - SLEW_MAX, speed + SLEW_MAX))
}

/// How the playing track is heard: the time it played,
/// that time weighted by how far the window was from the user (0 focused,
/// 0.5 in view behind another window, 1 hidden) where that is known, and
/// by the volume. Counted between the moments anything of it changes.
#[derive(Default)]
pub(super) struct Heard {
    since: Option<Instant>,
    played_ms: f64,
    known_ms: f64,
    away_ms: f64,
    volume_ms: f64,
}

impl Heard {
    /// Counting starts over (a new track starts).
    fn reset(&mut self) {
        *self = Self { since: Some(Instant::now()), ..Self::default() };
    }

    /// The time since the last tick played (or not) in this state.
    fn tick(&mut self, playing: bool, away: Option<f32>, volume: f32) {
        let now = Instant::now();
        let ms = self.since.map_or(0.0, |s| now.duration_since(s).as_secs_f64() * 1000.0);
        self.since = Some(now);
        self.add(ms, playing, away, volume);
    }

    fn add(&mut self, ms: f64, playing: bool, away: Option<f32>, volume: f32) {
        if !playing {
            return;
        }
        self.played_ms += ms;
        self.volume_ms += ms * f64::from(volume);
        if let Some(a) = away {
            self.known_ms += ms;
            self.away_ms += ms * f64::from(a);
        }
    }

    /// The attention (when the window's state covers half the time played
    /// at least) and the mean volume.
    fn take(&self) -> (Option<f32>, Option<f32>) {
        if self.played_ms < 1.0 {
            return (None, None);
        }
        let attention = (self.known_ms >= 0.5 * self.played_ms).then(|| (self.away_ms / self.known_ms) as f32);
        (attention, Some((self.volume_ms / self.played_ms) as f32))
    }
}

impl Worker {
    // ---------------------------------------------------------------- playback

    pub(crate) fn position_now(&self) -> Duration {
        self.position_at_instant(Instant::now())
    }

    /// Where we take playback to be at `now`, going from the last anchor at
    /// [`Self::rate`].
    pub(crate) fn position_at_instant(&self, now: Instant) -> Duration {
        if self.status == Status::Playing {
            self.position + now.saturating_duration_since(self.position_at).mul_f64(self.rate)
        } else {
            self.position
        }
    }

    /// Publishes only where playback is and how fast the view should move
    /// it (a position report, many a second: none of the rest changed).
    fn publish_clock(&mut self) {
        // The view shows another device's zone.
        if self.connect.remote() {
            return;
        }
        let now = Instant::now();
        let (position, rate) = (self.position_at_instant(now), self.rate);
        self.publish(|s| {
            s.player.position = position;
            s.player.at = now;
            s.player.rate = rate as f32;
        });
    }

    /// What to hand the engine for `e`: its best source that is usable and
    /// not in `skip` ([`Self::pick_source`]), with its gain and speed.
    /// Returns the source too; says in the log why nothing plays.
    pub(crate) fn request_skipping(
        &mut self,
        e: Entry,
        skip: &[SourceId],
    ) -> Option<(PlayRequest, SourceId, SourceKind)> {
        let src = self.pick_source(e, skip)?;
        self.turn.lap("source");
        let req = self.request_from(e, &src);
        self.turn.lap("gain");
        Some((req, src.id, src.kind))
    }

    /// The best source of `e` that is usable and not in `skip` (local >
    /// Spotify > YouTube, then quality). A stream source needs its service
    /// registered. A stream copy set aside earlier (its video refused once,
    /// its service leaving it out of an answer) is tried again when nothing
    /// else plays: a YouTube video unless it was refused in the last hours
    /// (`REFUSED_TTL`), another service's copy once the song cannot be
    /// looked up anywhere either. A listing that gives no full audio (a
    /// SoundCloud preview, a DRM-only stream) is never played. Says in the
    /// log why nothing plays.
    pub(crate) fn pick_source(&mut self, e: Entry, skip: &[SourceId]) -> Option<nikgob_core::model::Source> {
        let mut sources = match self.lib.sources_of_track(e.track) {
            Ok(s) => s,
            Err(err) => {
                log::warn!("{}: its sources could not be read: {err}", e.track);
                return None;
            }
        };
        let all = sources.len();
        sources.retain(|s| s.trashed_at.is_none() && !skip.contains(&s.id));
        sources.sort_by_key(|s| (!s.available, s.kind.priority(), std::cmp::Reverse(s.quality_score)));
        let refused = self.refused_videos();
        // A stream needs its service on and able to play sound for this
        // account (Spotify not connected, or before its playback login:
        // the library's Spotify tracks play from YouTube Music), and not
        // paused by a rate limit unless its audio is on disk.
        let mut lookup: Option<bool> = None;
        let mut usable = |w: &mut Self, s: &nikgob_core::model::Source| match s.kind.service() {
            None => s.available,
            Some(service) => {
                let ok = s.available
                    || if s.kind == SourceKind::Youtube {
                        !refused.contains(&s.locator)
                    } else {
                        w.lib.source_set_aside(s.id).unwrap_or(false)
                            && !*lookup.get_or_insert_with(|| w.can_look_up(e.track))
                    };
                ok && w.has_audio(service) && !w.waits_for_pause(service, &s.locator)
            }
        };
        let Some(src) = sources.iter().find(|s| usable(self, s)).cloned() else {
            // A file another device sent for it ([`connect`]).
            if let Some(src) = self.connect_source(e.track, skip) {
                return Some(src);
            }
            let why = if all == 0 {
                "it has no source".to_owned()
            } else if sources.is_empty() {
                format!("all its {all} sources were tried or trashed")
            } else if sources.iter().all(|s| !s.available && s.kind.service().is_none()) {
                "its files are missing".to_owned()
            } else if let Some(s) = sources.iter().find(|s| s.kind.service().is_some_and(|k| self.pause_left(k).is_some())) {
                format!("{} asks to wait", s.kind.as_str())
            } else {
                let services: Vec<&str> = sources.iter().filter_map(|s| s.kind.service()).map(|s| s.as_str()).collect();
                let off = if self.service(StreamKind::Youtube).is_none() { ", YouTube Music switched off" } else { "" };
                format!(
                    "no audio from {} now ({} sources set aside{off})",
                    services.join(", "),
                    sources.iter().filter(|s| !s.available).count()
                )
            };
            log::warn!("{}: nothing to play: {why}", e.track);
            return None;
        };
        if !src.available {
            log::info!("{}: trying its {} copy {} set aside earlier", e.track, src.kind.as_str(), src.locator);
        }
        Some(src)
    }

    /// The request of `e` from `src`: a file's gain read from its tags
    /// (once per file, [`convert::FactsCache`]; off the loop when that is
    /// slow, [`Self::file_gain`]), the length, the speed.
    fn request_from(&mut self, e: Entry, src: &nikgob_core::model::Source) -> PlayRequest {
        let duration = self
            .lib
            .get_track(e.track)
            .ok()
            .flatten()
            .and_then(|t| t.duration_ms)
            .map(|ms| Duration::from_millis(u64::from(ms)));
        let (source, gain_db) = match src.kind.service() {
            None => {
                let path = self.res.path(&src.locator);
                let gain = self.file_gain(e.id, &path);
                (PlaySource::File(path), gain)
            }
            Some(service) => (
                PlaySource::Stream {
                    service,
                    id: src.locator.clone(),
                },
                0.0,
            ),
        };
        let (speed, keep_timbre) = self.fx_of(e.track);
        PlayRequest {
            id: e.id,
            source,
            gain_db,
            duration,
            speed,
            keep_timbre,
        }
    }

    /// The version `track` plays as now, and whether it keeps its timbre.
    fn fx_of(&self, track: TrackId) -> (nikgob_view::SpeedVariant, bool) {
        let st = self.state.read();
        let variant = st.speeds.of(track);
        (variant, st.prefs.keep_timbre && variant != nikgob_view::SpeedVariant::Normal)
    }

    /// `Command::SetTrackSpeed`: remembered with the track, heard at once
    /// if it plays.
    pub(crate) fn set_track_speed(&mut self, track: TrackId, variant: nikgob_view::SpeedVariant) {
        // A speed of the listener's own that a ready version has is that
        // version, and one out of range is the nearest the engine takes.
        let variant = match variant {
            nikgob_view::SpeedVariant::Custom(percent) => nikgob_view::SpeedVariant::at(percent),
            ready => ready,
        };
        // The slider says the same speed again while it is held: nothing
        // to save and nothing to restart.
        if self.state.read().speeds.chosen.get(&track).copied().unwrap_or_default() == variant {
            return;
        }
        let key = (variant != nikgob_view::SpeedVariant::Normal).then(|| variant.key());
        if let Err(e) = self.lib.set_track_fx(track, key.as_deref()) {
            log::warn!("{track}: its speed could not be saved: {e}");
            return;
        }
        let mut chosen = (*self.state.read().speeds.chosen).clone();
        let mut names = (*self.state.read().speed_names).clone();
        if key.is_some() {
            chosen.insert(track, variant);
            if !names.contains_key(&track)
                && let Some(n) = speed_name(&self.lib, track)
            {
                names.insert(track, n);
            }
        } else {
            chosen.remove(&track);
            names.remove(&track);
        }
        self.publish(|s| {
            s.speeds.chosen = Arc::new(chosen);
            s.speed_names = Arc::new(names);
        });
        self.apply_speed(Some(track));
    }

    /// `Command::ResetTrackSpeed`: as recorded again, and «Отменить»
    /// brings its speed back.
    pub(crate) fn reset_track_speed(&mut self, track: TrackId) {
        let Some(was) = self.state.read().speeds.chosen.get(&track).copied() else { return };
        let title = self.state.read().speed_names.get(&track).map(|n| n.title.clone());
        self.set_track_speed(track, nikgob_view::SpeedVariant::Normal);
        let loc = self.locale();
        let text = match title {
            Some(t) => nikgob_core::i18n::misc_toasts::fmt_speed_as_recorded_in(loc, &t),
            None => nikgob_core::i18n::misc_toasts_in(loc).speed_reset.to_owned(),
        };
        let undo = self.undo_action(Command::SetTrackSpeed { track, variant: was });
        self.toast_info(text, Some(undo));
    }

    /// The speed of `changed` (or of everything: the settings) changed: the
    /// playing track goes on as its version now is, from where it is, and
    /// the preloaded next track, if it is concerned, takes its version in
    /// the engine. It is not handed over anew: near the end of the playing
    /// track the engine is already producing it, and a second copy would
    /// play after the first.
    pub(crate) fn apply_speed(&mut self, changed: Option<TrackId>) {
        if let Some(e) = self.playing {
            let fx = self.fx_of(e.track);
            if fx != self.playing_fx {
                self.playback.set_speed(e.id, fx.0, fx.1);
                self.playing_fx = fx;
                // The position is the track's own time: it stays, and
                // moves at the new pace from here.
                let now = Instant::now();
                self.position = self.position_at_instant(now);
                self.position_at = now;
                self.speed = f64::from(fx.0.speed());
                self.rate = self.speed;
                self.publish_player();
                self.prefer_ahead_again();
            }
        }
        let playing = self.playing.map(|e| e.id);
        if let Some(n) = self.queue.peek_auto_next()
            && Some(n.id) != playing
            && changed.is_none_or(|t| n.track == t)
        {
            let fx = self.fx_of(n.track);
            self.playback.set_speed(n.id, fx.0, fx.1);
        }
    }

    /// [`Self::request_skipping`] with nothing skipped.
    pub(crate) fn request(&mut self, e: Entry) -> Option<PlayRequest> {
        self.request_skipping(e, &[]).map(|(req, ..)| req)
    }

    /// Hands `req` (for the playing entry `e`) to the engine. A stream reads
    /// Buffering until the engine reports its start.
    fn play_request(&mut self, e: Entry, req: PlayRequest, source: SourceId, kind: SourceKind) {
        self.playing_fx = (req.speed, req.keep_timbre);
        self.speed = f64::from(req.speed.speed());
        self.sleep_before_play();
        self.playback.play(req);
        self.restarted = Some(e.id);
        self.playing = Some(e);
        self.playing_source = Some((source, kind));
        if kind == SourceKind::Youtube {
            self.youtube_plays();
        }
        self.status = if kind.is_local() {
            Status::Playing
        } else {
            Status::Buffering
        };
    }

    /// The source playing `e` did not open: tries its next source. Returns
    /// false when there is none left.
    pub(crate) fn try_next_source(&mut self, e: Entry) -> bool {
        if let Some((failed, _)) = self.playing_source.take() {
            self.tried.push(failed);
        }
        let skip = self.tried.clone();
        let Some((req, source, kind)) = self.request_skipping(e, &skip) else {
            // Last chance: the same song on YouTube Music.
            return self.wait_for_youtube(e);
        };
        log::info!("playing {} from its next source ({kind})", e.track);
        self.play_request(e, req, source, kind);
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        self.seek_to_failover(e);
        self.preload_next();
        self.publish_player();
        true
    }

    /// The entry `e` just started on another source after its last one
    /// failed mid-track: on from where that one stopped.
    pub(crate) fn seek_to_failover(&mut self, e: Entry) {
        let Some((_, at)) = self.failover.filter(|(id, _)| *id == e.id) else { return };
        log::info!("{}: going on at {} s", e.track, at.as_secs());
        self.playback.seek(at);
        self.position = at;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        self.publish_player();
    }

    /// A new list (an album, a playlist, a mix, a radio, a search result)
    /// is about to replace the queue: shuffle turns on or off as the
    /// setting of its `kind` says («Перемешивание»); a list of no kind (the
    /// library's tracks, a search, a radio) keeps the button as it is.
    pub(crate) fn new_list(&mut self, kind: Option<nikgob_view::ListKind>) {
        let rule = kind.map_or(nikgob_view::ListShuffle::Button, |k| self.state.read().prefs.list_shuffle.of(k));
        self.queue.new_list(rule, kind);
        // An API request said shuffled or not: so before the list is made.
        if let Some(on) = self.api.next_shuffle.take() {
            self.queue.shuffle = on;
        }
    }

    /// Writes the prefs a command changed without writing them itself, or
    /// whose last write failed ([`Self::prefs_unsaved`]). A failure that
    /// goes on is not said again.
    pub(crate) fn save_prefs_due(&mut self) {
        if self.prefs_unsaved {
            self.write_prefs_now(!self.prefs_failing);
        }
    }

    /// The kind of the library playlist `id` for [`Self::new_list`]: one of
    /// a service (followed, synced or imported from it) is a ready one;
    /// «Любимые» and those made here are the user's own.
    pub(crate) fn playlist_kind(&self, id: PlaylistId) -> nikgob_view::ListKind {
        if id != LIKED_PLAYLIST && self.lib.playlist_remote(id).ok().flatten().is_some() {
            nikgob_view::ListKind::ReadyPlaylist
        } else {
            nikgob_view::ListKind::OwnPlaylist
        }
    }

    /// «Перемешивать радио и автовоспроизведение»: a radio's, autoplay's
    /// and the daylist's picks play shuffled while shuffle is on.
    pub(crate) fn shuffle_radio(&self) -> bool {
        self.state.read().prefs.shuffle_radio
    }

    /// A list of tracks of the library's pages (downloaded, «Слушать
    /// снова», worlds, cards) from `start`, or from the first (a random one
    /// under shuffle) when the user picked none; shuffle stays as the
    /// button is.
    pub(crate) fn play_tracks(&mut self, tracks: &[TrackId], start: Option<usize>, origin: nikgob_view::QueueOrigin) {
        self.user_switch(EventOrigin::Manual);
        self.new_list(None);
        self.queue.set_from(tracks, start, origin);
        self.play_context = EventContext::Library;
        self.play_context_id = None;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn play_list(
        &mut self,
        ids: Vec<TrackId>,
        start: Option<TrackId>,
        shuffle: bool,
        kind: nikgob_view::ListKind,
        origin: nikgob_view::QueueOrigin,
        ctx: EventContext,
        context_id: Option<String>,
    ) {
        if ids.is_empty() {
            self.toast_nothing_to_play();
            return;
        }
        let start_idx = start.and_then(|s| ids.iter().position(|&t| t == s));
        self.user_switch(EventOrigin::Manual);
        self.turn.lap("switch");
        // «Перемешать» turns shuffle on; a plain play shuffles or not as
        // the setting of the list's kind says (shuffled: from a random
        // track unless one was picked).
        self.new_list(Some(kind));
        if shuffle {
            self.queue.shuffle = true;
        }
        self.queue.set_from(&ids, start_idx, origin);
        self.turn.lap("queue");
        // The new queue's own context, never the one before it (a mix's,
        // a playlist's) carried over.
        self.play_context = ctx;
        self.play_context_id = context_id;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    pub(crate) fn start(&mut self, e: Entry) {
        self.resume_at = None;
        // A place the user moved to while `e` waited for a lookup (or its
        // clip's song) is where it starts; anything else starts at 0:00.
        let waited = self.playing.is_some_and(|p| p.id == e.id) && self.playing_source.is_none();
        self.failover = self.failover.filter(|(id, _)| waited && *id == e.id);
        let mut e = e;
        // Skip forward over unplayable items rather than stall. With repeat on,
        // `skip_forward` wraps around, so the walk is bounded by the queue
        // length (otherwise a queue without a single playable file recurses
        // until the stack overflows).
        for attempt in 0..=self.queue.len() {
            self.tried.clear();
            // Disliked, where the app picked it: the next one.
            if self.skips_disliked(e) {
                log::info!("skipping the disliked {}", e.track);
                match self.queue.skip_forward() {
                    Some(n) if n.id != e.id => {
                        e = n;
                        continue;
                    }
                    _ => break,
                }
            }
            // A clip the radio or a mix queued: its song first, if it comes.
            if self.wait_for_clip_song(e) {
                return;
            }
            // From YouTube Music while Spotify plays: a moment for its
            // Spotify copy first.
            if self.wait_for_preferred(e) {
                return;
            }
            if let Some((req, source, kind)) = self.request_skipping(e, &[]) {
                self.start_with(e, req, source, kind);
                return;
            }
            // Its file is on its way from the device that sent the queue.
            if self.wait_for_pull(e) {
                return;
            }
            // Its service asks to wait: what plays now goes first, or the
            // player waits on it (never a run down the queue).
            if self.hold_for_pause(e) {
                return;
            }
            // Nothing plays now, but YouTube Music may have it: wait for it.
            if self.wait_for_youtube(e) {
                return;
            }
            log::warn!("no playable source for {}", e.track);
            // A guest of «✦ С дополнением» is passed over without a word.
            let guest = self.plus_failed(e.id);
            if attempt == 0 && !guest {
                // A stream track's last way to the sound is YouTube Music:
                // when it is off, that is the reason, not a missing file.
                let streamed = self
                    .lib
                    .sources_of_track(e.track)
                    .unwrap_or_default()
                    .iter()
                    .any(|s| s.trashed_at.is_none() && s.kind.service().is_some());
                let text = if streamed && self.service(StreamKind::Youtube).is_none() {
                    self.off_text(StreamKind::Youtube)
                } else {
                    nikgob_core::i18n::errors_in(self.locale()).no_file_for_track.to_owned()
                };
                self.toast_err(text, None);
            }
            match self.queue.skip_forward() {
                Some(n) if n.id != e.id => e = n,
                _ => break,
            }
        }
        self.stop();
    }

    /// Plays `e` from `req` and records the start (with the service that
    /// actually plays, for a stream).
    pub(crate) fn start_with(&mut self, e: Entry, req: PlayRequest, source: SourceId, kind: SourceKind) {
        self.play_request(e, req, source, kind);
        self.turn.lap("play");
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        // A stream a service's pause cut midway goes on from there, and a
        // track moved along while it waited from where it was moved to.
        if let Some(at) = self.take_held_position(e.id) {
            self.failover = Some((e.id, at));
        }
        self.seek_to_failover(e);
        self.record_start(e, (!kind.is_local()).then_some(kind));
        self.preload_next();
        self.turn.lap("next");
        // From YouTube Music while Spotify plays: its Spotify copy is
        // looked up for the next time (before the lookups ahead: one at a time).
        self.spotify_copy_for(e, kind);
        // What only YouTube Music may have is looked up ahead, gapless or not.
        self.resolve_ahead();
        self.turn.lap("ahead");
        self.publish_now_playing();
        self.turn.lap("now playing");
        self.likes_if_stale();
        self.turn.lap("likes");
        // The next files of a queue another device sent, fetched ahead.
        self.connect_prefetch();
    }

    pub(crate) fn preload_next(&mut self) {
        self.prepare_next(true);
        // The next streams' album covers, known before they start.
        self.complete_next();
    }

    /// [`Self::preload_next`]; `ahead`: a file whose tags were not read yet
    /// is read on another thread first.
    pub(crate) fn prepare_next(&mut self, ahead: bool) {
        // Gapless off: nothing is prepared, the next track starts after `Ended`.
        // Nor when the sleep timer ends with the playing track.
        let next = if self.state.read().prefs.gapless && !self.sleep_ends_with_playing() { self.queue.peek_auto_next() } else { None };
        // The engine holds this entry already: it keeps exactly what it was
        // handed, whatever has come since (a Spotify copy, a file). It may
        // have taken it over already, and a swap would record a source that
        // does not play, or open a second one.
        if next.is_some_and(|n| self.playback.holds_next(n.id)) {
            self.reading_ahead = None;
            return;
        }
        // A clip the app queued starts once its song is known (see
        // `streams::clips::QueueClips`): not handed over ahead.
        let next = next.filter(|n| !self.clip_not_ready(*n));
        // A stream a service's pause cut midway starts where it stopped,
        // through a start of its own (a gapless step would start it over).
        let next = next.filter(|n| !self.has_held_position(n.id));
        // Its Spotify copy is being looked up: handed over once that answer
        // is in, or a moment before the join (see `streams::prefer`).
        let next = next.filter(|n| !self.prefer_pending(*n));
        let Some((next, src)) = next.and_then(|n| Some((n, self.pick_source(n, &[])?))) else {
            self.reading_ahead = None;
            self.playback.set_next(None);
            return;
        };
        // A file whose tags were not read yet is read off the loop; the
        // engine gets the next track when they are.
        if ahead && src.kind.service().is_none() {
            let path = self.res.path(&src.locator);
            if self.res.facts().get(&path).is_none() && self.read_facts_ahead(path) {
                self.playback.set_next(None);
                return;
            }
        }
        // Prepared here: a read still on its way prepares nothing again.
        self.reading_ahead = None;
        let req = self.request_from(next, &src);
        self.playback.set_next_from(req, src.id, src.kind);
    }

    /// Reads the tags of `path` (the next track's file) on a thread of its
    /// own, then prepares the next track (whatever it is by then, never
    /// reading ahead again). False when no thread could start (it is read
    /// here then).
    fn read_facts_ahead(&mut self, path: PathBuf) -> bool {
        if self.reading_ahead.as_ref() == Some(&path) {
            return true;
        }
        if !self.read_facts_off_loop(&path) {
            return false;
        }
        self.reading_ahead = Some(path);
        true
    }

    pub(crate) fn pause(&mut self) {
        // Waiting out a service's pause: it stays paused now.
        self.cancel_pause_wake();
        if matches!(self.status, Status::Playing | Status::Buffering) {
            self.heard_tick();
            self.position = self.position_now();
            self.status = Status::Paused;
            self.playback.pause();
            self.publish_player();
        }
    }

    pub(crate) fn resume(&mut self) {
        match self.status {
            Status::Paused if self.playing.is_none() => self.start_restored(),
            Status::Paused => {
                self.heard_tick();
                self.position_at = Instant::now();
                self.rate = self.speed;
                self.status = Status::Playing;
                self.sleep_before_play();
                self.playback.resume();
                self.publish_player();
            }
            Status::Stopped => {
                if let Some(e) = self.queue.current() {
                    self.start(e);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn stop(&mut self) {
        self.playback.stop();
        self.playing = None;
        self.playing_source = None;
        self.failover = None;
        self.status = Status::Stopped;
        self.position = Duration::ZERO;
        self.publish_now_playing();
    }

    /// The skipped track stops and the player waits, buffering, for what
    /// the radio is picking ([`Self::radio_holds`]).
    pub(crate) fn wait_for_radio(&mut self) {
        // Nothing current: the first pick that comes is (repeat is off,
        // or «Дальше» would have wrapped).
        let _ = self.queue.advance_auto();
        self.stop();
        self.status = Status::Buffering;
        self.publish_player();
    }

    pub(crate) fn seek(&mut self, p: Duration) {
        if self.playing.is_none() {
            // The restored track: play will start from here.
            if self.resume_at.is_some() {
                if let Some(c) = self.queue.current() {
                    self.drop_held_position(c.id);
                }
                self.resume_at = Some(p);
                self.position = p;
                self.publish_player();
            }
            return;
        }
        let from = self.position_now();
        match self.playing {
            // Waiting for a lookup (or a clip's song): nothing is open to
            // seek in; it starts from here (see `seek_to_failover`).
            Some(e) if self.playing_source.is_none() => self.failover = Some((e.id, p)),
            _ => self.playback.seek(p),
        }
        self.position = p;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        if let Some(e) = self.playing {
            let actor = self.hand_actor();
            let _ = self.lib.record_event_with(
                e.track,
                EventType::Seek,
                None,
                Some(p.as_millis() as u32),
                self.play_context,
                EventMeta { actor, ..EventMeta::default() },
            );
            if self.queue_teaches() && teaches_live_recs(actor) {
                self.recs_seeked(e.track, from.as_millis() as u32, p.as_millis() as u32);
            }
        }
        self.publish_player();
        // Nearer the end now: the next track's wait for its Spotify copy is
        // counted again.
        self.prefer_ahead_again();
    }

    /// Who chose `e`, starting now: what the command said, else the user
    /// when the queue is another one, else the radio or autoplay that
    /// added it, else the queue going on.
    fn origin_of(&mut self, e: Entry) -> EventOrigin {
        let generation = self.queue.generation();
        let new_queue = self.origin_gen != Some(generation);
        self.origin_gen = Some(generation);
        if new_queue && self.autoplay_gen != Some(generation) {
            self.autoplay_entries.clear();
        }
        if let Some(o) = self.next_origin.take() {
            return o;
        }
        if let Some(o) = self.radio_origin(e.id) {
            return o;
        }
        if self.autoplay_owns(e.id) {
            return EventOrigin::Autoplay;
        }
        if new_queue { EventOrigin::Manual } else { EventOrigin::Queue }
    }

    /// Records the start of `e` with who chose it and what plays it.
    fn record_start(&mut self, e: Entry, kind: Option<SourceKind>) {
        let origin = self.origin_of(e);
        // What the radio or autoplay added plays in it, even in the queue
        // of an album it went on after.
        let (context, context_id) = match self.radio_context(e.id) {
            Some((ctx, id)) => (ctx, Some(id)),
            None => (self.play_context, self.play_context_id.clone()),
        };
        self.ended = false;
        self.heard.reset();
        // Who plays it: whoever added or picked it, else whoever's queue it is.
        self.play_actor = self.entry_actor.get(&e.id).copied().unwrap_or(self.queue_actor);
        if self.entry_actor.len() > ENTRY_ACTORS_KEPT {
            let live: std::collections::HashSet<u64> =
                self.queue.history().iter().chain(self.queue.upcoming()).map(|x| x.id).chain([e.id]).collect();
            self.entry_actor.retain(|id, _| live.contains(id));
        }
        let meta = EventMeta { origin: Some(origin), context_id, actor: self.play_actor, ..EventMeta::default() };
        let _ = self.lib.record_event_with(e.track, EventType::Start, kind, Some(0), context, meta);
        self.turn.lap("start recorded");
        if self.queue_teaches() {
            self.recs_started(e.track, origin);
        }
        self.turn.lap("recs session");
        self.home_input_changed();
    }

    /// The playing track's [`Heard`] counts the time since its last tick,
    /// in the state the player and the window are in.
    pub(crate) fn heard_tick(&mut self) {
        let playing = self.status == Status::Playing;
        let away = self.window_away();
        let volume = if self.muted { 0.0 } else { self.volume };
        self.heard.tick(playing, away, volume);
    }

    /// The actor of the API request that runs now (its token's), `None`
    /// the owner (an owner's token, the window).
    pub(crate) fn api_actor(&self) -> Option<nikgob_core::settings::ApiActor> {
        self.api.actor.filter(|a| *a != nikgob_core::settings::ApiActor::Owner)
    }

    /// The user picks something else to play (`origin` of what comes): the
    /// playing track ends here.
    pub(crate) fn user_switch(&mut self, origin: EventOrigin) {
        if !self.ended {
            self.record_end(true, EndReason::UserPick);
        }
        self.next_origin = Some(origin);
        // Whoever asked for the new queue (an API request's token while its
        // commands run; the window: the owner) plays it from now on.
        self.queue_actor = self.api_actor();
    }

    /// The user picks another entry of the queue that plays (`origin` of
    /// what comes): the playing track ends here, the picked `entry` plays
    /// as whoever picked it, and the queue stays whose it was (after a
    /// guest's pick in the owner's queue the next song is the owner's).
    pub(crate) fn user_pick(&mut self, origin: EventOrigin, entry: u64) {
        if !self.ended {
            self.record_end(true, EndReason::UserPick);
        }
        self.next_origin = Some(origin);
        let by = self.api_actor();
        if by == self.queue_actor {
            self.entry_actor.remove(&entry);
        } else {
            self.entry_actor.insert(entry, by);
        }
    }

    /// Whether the play that plays teaches the live recommendations (the
    /// session, the skip and next-start models): the owner's and an
    /// agent's do; a guest's never, the voice assistant's are background
    /// (as in the history the mixes are made of).
    fn queue_teaches(&self) -> bool {
        teaches_live_recs(self.play_actor)
    }

    /// Who ends or seeks the song that plays: an API request's token
    /// while its commands run (the voice assistant's «дальше» is its own;
    /// an owner's token is the owner, as the window is), else whoever
    /// plays it.
    fn hand_actor(&self) -> Option<nikgob_core::settings::ApiActor> {
        self.api_actor().or(self.play_actor)
    }

    /// The app closes: the playing (or paused) track ends as it is.
    pub(crate) fn record_closed(&mut self) {
        if !self.ended && self.playing.is_some() {
            self.record_end(true, EndReason::AppClosed);
        }
    }

    /// Records how the current track ended (`reason`): a skip before 90 %,
    /// else complete; once per start.
    pub(crate) fn record_end(&mut self, by_user: bool, reason: EndReason) {
        let Some(e) = self.playing else { return };
        if self.ended {
            return;
        }
        self.ended = true;
        if let Some((source, SourceKind::Youtube)) = self.playing_source {
            self.analyse_played(e.track, source);
        }
        let pos = self.position_now();
        let dur = self
            .lib
            .get_track(e.track)
            .ok()
            .flatten()
            .and_then(|t| t.duration_ms);
        let complete = !by_user
            || dur.is_some_and(|d| pos.as_millis() as f64 >= f64::from(d) * 0.9);
        let position_ms = pos.as_millis() as u32;
        // Who ended it: a guest's «дальше» on the owner's song is the
        // guest's, never the owner's skip.
        let actor = self.hand_actor();
        let guest = Some(nikgob_core::settings::ApiActor::Guest);
        if reason != EndReason::AppClosed {
            if self.queue_teaches() && teaches_live_recs(actor) {
                self.recs_played();
                self.recs_ended(e.track, complete, position_ms, dur);
            }
            // After the session: the radio reads its verdict (a radio the
            // assistant started still follows its skips; a guest's plays
            // and skips steer nothing).
            if self.play_actor != guest && actor != guest {
                self.radio_feedback(e.track, complete, position_ms, dur);
            }
            // «Дальше» on a guest is a skip at any point of it.
            self.plus_ended(e.id, reason == EndReason::UserNext || (by_user && !complete));
        }
        self.heard_tick();
        let (attention, volume) = self.heard.take();
        let meta = EventMeta { end_reason: Some(reason), attention, volume, actor, ..EventMeta::default() };
        let _ = self.lib.record_event_with(
            e.track,
            if complete {
                EventType::Complete
            } else {
                EventType::Skip
            },
            None,
            Some(position_ms),
            self.play_context,
            meta,
        );
    }

    pub(crate) fn playback_event(&mut self, ev: PlaybackEvent) {
        // How long a position report waited to be taken (a busy worker can
        // hold it for a while): the sound went on meanwhile.
        let (ev, late) = match ev {
            PlaybackEvent::PositionAt { id, position, at } => (PlaybackEvent::Position { id, position }, at.elapsed().min(POSITION_SNAP)),
            ev => (ev, Duration::ZERO),
        };
        self.api_audio_event(&ev);
        match ev {
            PlaybackEvent::Started { id } => {
                if self.restarted == Some(id) {
                    self.restarted = None;
                }
                if self.playing.map(|e| e.id) == Some(id) {
                    if self.status == Status::Buffering {
                        self.heard_tick();
                        self.status = Status::Playing;
                    }
                    // A failover starts where the failed source stopped.
                    self.position = match self.failover.take() {
                        Some((failed, at)) if failed == id => at,
                        _ => Duration::ZERO,
                    };
                    self.position_at = Instant::now();
                    self.rate = self.speed;
                    self.publish_player();
                }
            }
            PlaybackEvent::TrackChanged { to } => {
                // What the engine took over: the source it was handed.
                let handed = self.playback.take_next(to);
                // `play()` over a playing item also reports a change: that one
                // we started ourselves. The same entry changing to itself
                // otherwise is repeat-one looping gaplessly: handled below
                // like any auto-advance, so the position starts over.
                if self.restarted.take() == Some(to) {
                    if self.status == Status::Buffering {
                        self.heard_tick();
                        self.status = Status::Playing;
                        self.publish_player();
                    }
                    return;
                }
                if self.queue.peek_auto_next().map(|e| e.id) != Some(to) {
                    log::debug!("backend changed to {to}, not the queued next");
                    return;
                }
                // The sleep timer ended with the one that ended.
                let sleep_boundary = self.sleep_ends_with_playing();
                self.record_end(false, EndReason::Ended);
                if let Some(e) = self.queue.advance_auto() {
                    self.playing = Some(e);
                    self.tried.clear();
                    // The engine took over what `preload_next` asked for.
                    let taken = handed.filter(|(req, ..)| req.id == e.id).or_else(|| self.request_skipping(e, &[]));
                    self.playing_fx = taken
                        .as_ref()
                        .map_or((nikgob_view::SpeedVariant::Normal, false), |(req, ..)| (req.speed, req.keep_timbre));
                    self.speed = f64::from(self.playing_fx.0.speed());
                    self.playing_source = taken.map(|(_, source, kind)| (source, kind));
                    self.position = Duration::ZERO;
                    self.position_at = Instant::now();
                    self.rate = self.speed;
                    let kind = self.playing_source.map(|(_, k)| k).filter(|k| !k.is_local());
                    self.record_start(e, kind);
                    self.preload_next();
                    self.publish_now_playing();
                    if sleep_boundary {
                        self.sleep_crossed();
                    }
                }
            }
            PlaybackEvent::Position { id, position } => {
                // The engine plays it: a start report that never came (or
                // was taken for another item) must not leave the player in
                // Buffering, where the view freezes the position and the
                // lyrics stop following the words.
                if self.playing.map(|e| e.id) == Some(id)
                    && self.status == Status::Buffering
                    && position > Duration::ZERO
                {
                    log::info!("item {id} plays without a start report; playing now");
                    self.restarted = None;
                    self.heard_tick();
                    self.status = Status::Playing;
                    self.position = position;
                    self.position_at = Instant::now();
                    self.rate = self.speed;
                    self.publish_player();
                    return;
                }
                if self.playing.map(|e| e.id) == Some(id) {
                    let stalled = self.reported == Some((id, position));
                    self.reported = Some((id, position));
                    if self.status != Status::Playing {
                        self.position = position;
                        self.position_at = Instant::now();
                        self.rate = self.speed;
                        return;
                    }
                    // The view extrapolates from its last publish and stops at
                    // the track's end, so it follows what is heard: a small
                    // difference is made up by running a little faster or
                    // slower (never backwards); a far one (a repeat-one loop,
                    // a seek we did not hear about, a long stall) is jumped.
                    let now = Instant::now();
                    let ours = self.position_at_instant(now);
                    // Where the sound is now (a stalled one stands still).
                    let heard = if stalled { position } else { position + late.mul_f64(self.speed) };
                    match follow_at(ours, heard, stalled, self.rate, self.speed) {
                        Follow::Keep => {}
                        Follow::Snap => {
                            if self.jumped_at.elapsed() < POSITION_SETTLE {
                                return;
                            }
                            self.position = heard;
                            self.position_at = now;
                            self.rate = if stalled { 0.0 } else { self.speed };
                            self.publish_player();
                        }
                        Follow::Rate(rate) => {
                            self.position = ours;
                            self.position_at = now;
                            self.rate = rate;
                            self.publish_clock();
                        }
                    }
                }
            }
            // Taken as a position above.
            PlaybackEvent::Paused | PlaybackEvent::Resumed | PlaybackEvent::PositionAt { .. } => {}
            PlaybackEvent::PausedElsewhere { id } => {
                if self.playing.map(|e| e.id) == Some(id) {
                    log::info!("item {id} was paused from elsewhere; paused here");
                    self.pause();
                }
            }
            PlaybackEvent::Speed { id, speed } => {
                let speed = f64::from(speed);
                if self.playing.map(|e| e.id) == Some(id) && speed.is_finite() && speed > 0.0 && speed != self.speed {
                    let now = Instant::now();
                    self.position = self.position_at_instant(now);
                    self.position_at = now;
                    self.speed = speed;
                    self.rate = speed;
                    self.publish_player();
                    self.prefer_ahead_again();
                }
            }
            PlaybackEvent::DeviceError { generation, message } => {
                log::error!("audio output: {message}");
                // A switch whose device did not open: the old one plays on.
                self.playback.drop_device(generation);
                // The same message while one is up only starts it over.
                self.toast_err(nikgob_core::i18n::errors_in(self.locale()).audio_output_failed, None);
            }
            PlaybackEvent::DeviceReady { generation } => self.device_ready(generation),
            PlaybackEvent::Duration { id, duration } => {
                // The library's length wins; the engine's is for a track
                // without one (a stream whose service did not say).
                let Some(e) = self.playing.filter(|e| e.id == id) else { return };
                let ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
                if ms > 0 && self.lib.fill_duration(e.track, ms).unwrap_or(false) {
                    log::debug!(target: "nikgob_runtime::streams", "{}: its length {ms} ms is the heard one", e.track);
                    self.publish_now_playing();
                }
            }
            PlaybackEvent::Quality { id, label } => {
                log::debug!(target: "nikgob_runtime::streams", "item {id} plays as {}", label.text_in(self.locale()));
                self.qualities.retain(|(i, _)| *i != id);
                if self.qualities.len() >= 4 {
                    self.qualities.remove(0);
                }
                self.qualities.push((id, label));
                if self.playing.map(|e| e.id) == Some(id) {
                    // A copy set aside earlier plays after all: it counts again.
                    if let Some((source, kind)) = self.playing_source.filter(|(_, k)| !k.is_local())
                        && self.lib.get_source(source).ok().flatten().is_some_and(|s| !s.available)
                    {
                        log::info!("{kind} copy {source} plays again");
                        if let Err(e) = self.lib.set_source_available(source, true) {
                            log::warn!("{source}: {e}");
                        }
                    }
                    self.publish_now_playing();
                }
            }
            PlaybackEvent::Ended { id } => {
                self.playback.let_go(id);
                if self.playing.map(|e| e.id) == Some(id) {
                    self.record_end(false, EndReason::Ended);
                    if self.sleep_ended(id) {
                        return;
                    }
                    // Repeat-one with no preload (or preload failed): restart.
                    match self.queue.advance_auto() {
                        Some(e) => self.start(e),
                        None => {
                            self.stop();
                            // A radio still picking its next tracks plays on.
                            self.radio_ran_out();
                        }
                    }
                }
            }
            PlaybackEvent::Error { id, message } => {
                log::error!("playback error on {id}: {message}");
                self.playback.let_go(id);
                let playing = self.playing.filter(|e| e.id == id);
                if playing.is_none() && self.queue.peek_auto_next().is_some_and(|n| n.id == id) {
                    // The preloaded next track: it is tried again (with
                    // all its sources) when its turn comes.
                    return;
                }
                if playing.is_none() {
                    // A track no longer playing (stopped, skipped, replaced
                    // while its stream still answered): not this one's
                    // failure, no skip, no word, no mark on its source.
                    log::info!("error of item {id}, which no longer plays: {message}");
                    return;
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && kind == SourceKind::Soundcloud
                    && message.contains(OFFICIAL_PLAYER_ROUTE)
                    && self.official_player_tried != Some(e.id)
                {
                    // Only DRM streams: the backend plays the same source
                    // through SoundCloud's own player now.
                    self.official_player_tried = Some(e.id);
                    let skip = self.tried.clone();
                    if let Some((req, again, kind)) = self.request_skipping(e, &skip).filter(|(_, s, _)| *s == source) {
                        log::info!("{}: SoundCloud has it only with DRM, playing it through SoundCloud's player", e.track);
                        self.play_request(e, req, again, kind);
                        self.seek_to_failover(e);
                        return;
                    }
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && kind == SourceKind::Youtube
                    && youtube_refused_midway(&message)
                    && !self.position.is_zero()
                    && self.stream_retried != Some(e.id)
                {
                    // YouTube refused the stream's URLs after it played a
                    // while (the ~15 s 403): a new stream asks the player
                    // afresh and goes on from here, once.
                    self.stream_retried = Some(e.id);
                    self.failover = Some((e.id, self.position));
                    let skip = self.tried.clone();
                    if let Some((req, again, kind)) = self.request_skipping(e, &skip).filter(|(_, s, _)| *s == source) {
                        log::info!("{}: YouTube refused the stream midway, opening it again", e.track);
                        self.play_request(e, req, again, kind);
                        self.seek_to_failover(e);
                        return;
                    }
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && kind.service().is_some()
                    && message.contains("rate limited")
                {
                    // The service paused (as the stream opened, or as it
                    // asked for its URL again): not this track's failure,
                    // and no lesson (see `streams::held`). It goes on from
                    // where it stopped: on another copy that plays now, or
                    // whenever its entry starts again.
                    let at = self.position;
                    if !at.is_zero() {
                        self.failover = Some((e.id, at));
                    }
                    self.tried.push(source);
                    let skip = self.tried.clone();
                    if let Some((req, again, other)) = self.request_skipping(e, &skip) {
                        log::info!("{}: {kind} asks to wait; its {other} copy plays", e.track);
                        self.playing_source = None;
                        self.play_request(e, req, again, other);
                        self.position = Duration::ZERO;
                        self.position_at = Instant::now();
                        self.rate = self.speed;
                        self.jumped_at = Instant::now();
                        self.seek_to_failover(e);
                        self.preload_next();
                        self.publish_player();
                        return;
                    }
                    let ended = std::mem::replace(&mut self.ended, true);
                    self.playing_source = None;
                    self.hold_position(e.id, at);
                    if self.hold_for_pause(e) {
                        return;
                    }
                    self.drop_held_position(e.id);
                    self.ended = ended;
                    self.playing_source = Some((source, kind));
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && !self.stream_failed(e.track, source, &message)
                    && kind == SourceKind::Youtube
                {
                    // YouTube's failure, not the video's: its other copies
                    // would hang the same way.
                    let copies = self.lib.sources_of_track(e.track).unwrap_or_default();
                    self.tried
                        .extend(copies.iter().filter(|s| s.kind == SourceKind::Youtube).map(|s| s.id));
                }
                // Where the engine had got to (what it last reported, not
                // the extrapolation: a stream that runs dry stands still).
                if let Some(e) = playing
                    && !self.position.is_zero()
                {
                    self.failover = Some((e.id, self.position));
                }
                if let Some(e) = playing
                    && self.try_next_source(e)
                {
                    return;
                }
                let title = playing
                    .and_then(|e| self.lib.get_track(e.track).ok().flatten())
                    .map(|t| t.title);
                let why = playing.and_then(|e| self.streaming.failures.remove(&e.track));
                // A guest of «✦ С дополнением» is passed over without a word.
                let guest = playing.is_some_and(|e| self.plus_failed(e.id));
                if !guest {
                    let (msg, action) = playback_error_toast(self.locale(), title, why);
                    match action {
                        Some((label, link)) => self.toast_open(nikgob_view::ToastKind::Error, msg, label, link),
                        None => self.toast_err(msg, None),
                    }
                }
                if playing.is_some() {
                    match self.queue.skip_forward() {
                        Some(e) if e.id != id => self.start(e),
                        _ => self.stop(),
                    }
                }
            }
        }
    }

    /// What the playing stream is: what it said once opened, else what its
    /// service usually streams. `None` for a local file.
    fn audio_quality(&self) -> Option<String> {
        let service = self.playing_source.and_then(|(_, k)| k.service())?;
        let id = self.playing?.id;
        let known = self.qualities.iter().find(|(i, _)| *i == id).map(|(_, q)| q.text_in(self.locale()));
        Some(known.unwrap_or_else(|| service.nominal_quality().words_in(self.locale())))
    }

    pub(crate) fn publish_player(&mut self) {
        // «✦ С дополнением»: a guest for a place that came near.
        self.plus_top_up();
        // A radio picks more when its queue runs low.
        self.radio_top_up();
        // The view shows another device's zone: this one is the parked view.
        if self.connect.remote() {
            self.publish_parked();
            return;
        }
        let radio = self.radio_view();
        let status = self.status;
        let now = Instant::now();
        let (position, rate) = (self.position_at_instant(now), self.rate as f32);
        let speed = self.speed as f32;
        let (volume, muted) = (self.volume, self.muted);
        let (shuffle, repeat) = (self.queue.shuffle, self.queue.repeat);
        let origin = self.queue.origin.clone();
        let page = self.context_page_id().and_then(context_page);
        let upcoming: Vec<Entry> = self.queue.upcoming().iter().take(200).copied().collect();
        let history = self.queue.history();
        let history: Vec<Entry> = history[history.len().saturating_sub(HISTORY_SHOWN)..].to_vec();
        let current_entry = self.queue.current().map(|e| e.id);
        let audio_service = self.playing_source.and_then(|(_, k)| k.service());
        let audio_quality = self.audio_quality();
        let soloist = audio_service == Some(StreamKind::Spotify) && self.soloist_ready();
        // The rows of both lists in one query, before the lock.
        let ids: Vec<TrackId> = upcoming.iter().chain(&history).map(|e| e.track).collect();
        let found = self.lib.get_track_rows(&ids).unwrap_or_default();
        let loc = self.locale();
        let why = &self.recs.why;
        let current_why = current_entry.and_then(|e| why.get(&e)).map(|w| w.get(loc).clone());
        let guests = self.recs.plus.guests();
        let current_guest = current_entry.is_some_and(|e| guests.contains(&e));
        let res = &mut self.res;
        let mut rows = |list: &[Entry]| -> Vec<nikgob_view::QueueEntry> {
            list.iter()
                .filter_map(|e| {
                    found.get(&e.track).map(|r| nikgob_view::QueueEntry {
                        entry: e.id,
                        track: res.track_in(r, loc),
                        why: why.get(&e.id).map(|w| w.get(loc).clone()),
                        guest: guests.contains(&e.id),
                    })
                })
                .collect()
        };
        let next = rows(&upcoming);
        let prev = rows(&history);
        self.publish(|s| {
            let p = &mut s.player;
            p.status = status;
            p.audio_service = audio_service;
            p.audio_quality = audio_quality;
            p.soloist = soloist;
            p.position = position;
            p.at = now;
            p.rate = rate;
            p.speed = speed;
            p.volume = volume;
            p.muted = muted;
            p.shuffle = shuffle;
            p.repeat = repeat;
            p.set_context(origin, page, loc);
            s.queue.next = next;
            s.queue.prev = prev;
            s.queue.current_entry = current_entry;
            s.queue.current_why = current_why;
            s.queue.current_guest = current_guest;
            s.queue.radio = radio;
        });
    }

    /// The id of what the queue was started from (see
    /// [`Self::play_context_id`]); after a restart, the one the last run
    /// left, while its queue plays.
    pub(crate) fn context_page_id(&self) -> Option<&str> {
        let restored = self.restored_page.as_ref().filter(|(g, _)| *g == self.queue.generation());
        self.play_context_id.as_deref().or(restored.map(|(_, id)| id.as_str()))
    }

    /// Volume and mute only: nothing else of the player or the queue changes.
    pub(crate) fn publish_volume(&mut self) {
        if self.connect.remote() {
            return;
        }
        let (volume, muted) = (self.volume, self.muted);
        self.publish(|s| {
            s.player.volume = volume;
            s.player.muted = muted;
        });
    }

    pub(crate) fn publish_now_playing(&mut self) {
        // The view shows another device's zone: this one is the parked view.
        if self.connect.remote() {
            self.publish_parked();
            return;
        }
        let shown = self.playing.or_else(|| self.resume_at.and(self.queue.current()));
        let current = shown
            .and_then(|e| self.lib.get_track_row(e.track).ok().flatten());
        // The source that plays (a Spotify track may sound from YouTube),
        // else the track's best one.
        let playing = self
            .playing_source
            .filter(|_| self.playing.is_some() && self.playing == shown)
            .and_then(|(id, _)| self.lib.get_source(id).ok().flatten());
        let src = playing.or_else(|| shown.and_then(|e| self.lib.active_source(e.track).ok().flatten()));
        let audio_quality = self.audio_quality();
        let changed = self.state.read().player.current.as_ref().map(|r| r.id)
            != current.as_ref().map(|r| r.id);
        let loc = self.state.read().prefs.resolved_locale();
        let row = current.as_ref().map(|r| self.res.track_in(r, loc));
        let mut info = self.res.info_in(current.as_ref(), src.as_ref(), loc);
        if !changed
            && let Some(s) = src.as_ref().filter(|s| s.kind.is_local())
            && let Some(tags) = self.res.facts().get(&self.res.path(&s.locator)).and_then(|f| f.tags)
        {
            info.tags.extend(crate::import::extra_tags_in(&tags, loc));
        }
        if audio_quality.is_some() {
            // «Качество» of a stream: what it gives.
            if let (Some(d), Some(q)) = (info.sound.as_mut(), audio_quality.as_ref()) {
                d.facts.insert(0, (nikgob_core::i18n::player_in(loc).fact_stream.into(), q.clone()));
            }
            info.quality = audio_quality;
        }
        self.publish(|s| {
            s.player.duration = row.as_ref().and_then(|r| r.duration);
            s.player.current = row.clone();
            s.queue.current = row;
            s.info = info;
        });
        if changed && let Some(t) = current.as_ref().map(|r| r.id) {
            self.track_details(t);
            self.kick_better_covers(Some(t));
            self.complete_playing(t);
        }
        self.publish_player();
        // The info window of another track follows the library too.
        self.refresh_track_info();
    }
}

/// A playback error that is YouTube refusing a stream's URLs (every client
/// asked, HTTP 403), not the video's own refusal or a lost connection.
fn youtube_refused_midway(message: &str) -> bool {
    let m = message.to_lowercase();
    (m.contains("refused") || m.contains("http 403")) && !streams::resolve::video_refused(message)
}

/// Whether the plays of a queue started by `actor` (`None`: the owner, in
/// the window) teach the live recommendations ([`Worker::queue_teaches`]).
pub(crate) fn teaches_live_recs(actor: Option<nikgob_core::settings::ApiActor>) -> bool {
    !matches!(actor, Some(nikgob_core::settings::ApiActor::Guest | nikgob_core::settings::ApiActor::Assistant))
}

/// Formats a playback failure toast and optional settings button for `loc`.
pub(super) fn playback_error_toast(
    loc: nikgob_core::i18n::Locale,
    title: Option<String>,
    why: Option<(String, Option<nikgob_view::SettingLink>)>,
) -> (String, Option<(&'static str, nikgob_view::Link)>) {
    let err = nikgob_core::i18n::errors_in(loc);
    match (title, why) {
        (Some(t), Some((why, Some(link)))) => (
            format!("{t}: {why}"),
            Some((err.sign_in, nikgob_view::Link::Setting(link))),
        ),
        (Some(t), Some((why, None))) => (format!("{t}: {why}"), None),
        _ => (err.playback_failed.to_owned(), None),
    }
}
