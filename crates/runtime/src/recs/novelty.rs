//! How new music is received (recs-v2 §11.4), in the runtime: every start
//! of a new song of a service (not the library's, picked by a mix, a radio,
//! autoplay or «Новое для тебя») and how it ended go into the
//! [`NewLog`] the library's cache keeps (`recs.new_log`), with where it was
//! found and its analysed preview. A new song played to the end twice is
//! offered once to be kept: ♡ «Оставить у себя», a like, which downloads
//! it. Its like is the «рекомендованный лайк» the log keeps too. The skip
//! model learns from the starts of new songs as of the library's, with its
//! «new from a service» feature.

use nikgob_core::model::{EventContext, EventOrigin, LikeTarget, TrackId};
use nikgob_library::now_ms;
use nikgob_recs::{Ended, Heard, NewEvent, NewLog, Source, Vector, song_key};
use nikgob_view::Command;

use super::recs_now;
use crate::Worker;

const LOG_KEY: &str = "recs.new_log";

/// The log as the library keeps it.
pub(super) fn log_of(lib: &nikgob_library::Library) -> NewLog {
    super::cache_read::<NewLog>(lib, LOG_KEY).unwrap_or_default()
}

/// A new song playing.
#[derive(Clone)]
struct Playing {
    track: TrackId,
    song: String,
    artist: String,
    title: String,
    source: Option<Source>,
    vector: Option<Vector>,
}

#[derive(Default)]
pub(crate) struct NewState {
    loaded: bool,
    pub log: NewLog,
    playing: Option<Playing>,
}

impl Worker {
    pub(crate) fn new_log_load(&mut self) {
        if self.recs.newlog.loaded {
            return;
        }
        self.recs.newlog.loaded = true;
        self.recs.newlog.log = log_of(&self.lib);

    }

    fn new_log_save(&self) {
        super::cache_write(&self.lib, LOG_KEY, now_ms(), &self.recs.newlog.log);
    }

    /// `track` is a new song of a service the app or «Новое для тебя»
    /// brought: what the log keeps of it.
    fn new_song(&self, track: TrackId, origin: EventOrigin) -> Option<Playing> {
        let snap = self.snapshot()?;
        if snap.catalog.get(track).is_some() {
            return None;
        }
        let row = self.lib.get_track_row(track).ok().flatten()?;
        let song = song_key(&row.title, &row.artist);
        if snap.filter.known_songs.contains(&song) || !(origin.is_auto() || self.play_context == EventContext::HomeSection) {
            return None;
        }
        Some(Playing {
            track,
            artist: nikgob_recs::artist_key(&row.artist),
            title: row.title.clone(),
            source: self.discovery_source(&song),
            vector: self.analysis.index().read().get(track).map(|v| snap.model.embed(v)),
            song,
        })
    }

    /// A start: a new song is remembered until it ends, and the skip model
    /// sees its features.
    pub(crate) fn new_started(&mut self, track: TrackId, origin: EventOrigin) -> Option<[f32; nikgob_recs::model::G]> {
        self.new_log_load();
        let playing = self.new_song(track, origin);
        let x = playing.as_ref().and_then(|p| {
            let snap = self.snapshot()?;
            let m = &snap.model;
            let moment = m.moment(&self.recs.session, recs_now());
            let duration = self.lib.get_track(track).ok().flatten().and_then(|t| t.duration_ms);
            Some(m.skip_features_new(&moment, p.vector.as_ref(), duration, self.play_context))
        });
        self.recs.newlog.playing = playing;
        x
    }

    /// How a start ended: a new song's outcome goes into the log; heard
    /// out twice, it is offered to be kept.
    pub(crate) fn new_ended(&mut self, track: TrackId, ended: Ended) {
        let Some(p) = self.recs.newlog.playing.take_if(|p| p.track == track) else { return };
        let heard = match ended {
            Ended::Complete => Heard::Complete,
            Ended::Early => Heard::Early,
            Ended::Late | Ended::Open => return,
        };
        self.push_new(&p, heard);
        if heard == Heard::Complete && self.recs.newlog.log.keep_offer_due(&p.song) {
            self.recs.newlog.log.offered.insert(p.song.clone());
            self.new_log_save();
            log::info!("new music: «{}» heard out twice, offered to keep", p.title);
            self.toast(
                format!("Оставить у себя «{}»?", p.title),
                Some(("♡ Оставить".into(), Command::Like(LikeTarget::Track(track), true))),
            );
        }
    }

    /// A like of `track`: a new song's like is a recommended like.
    pub(crate) fn new_liked(&mut self, track: TrackId) {
        self.new_log_load();
        let p = match self.recs.newlog.playing.clone().filter(|p| p.track == track) {
            Some(p) => Some(p),
            None => self.new_song(track, EventOrigin::Mix),
        };
        if let Some(p) = p {
            self.recs.newlog.log.offered.insert(p.song.clone());
            self.push_new(&p, Heard::Liked);
            log::info!("new music: «{}» liked", p.title);
        }
    }

    fn push_new(&mut self, p: &Playing, heard: Heard) {
        self.recs.newlog.log.push(NewEvent {
            song: p.song.clone(),
            artist: p.artist.clone(),
            source: p.source,
            vector: p.vector,
            track: Some(p.track),
            at: recs_now(),
            heard,
        });
        self.new_log_save();
    }
}
