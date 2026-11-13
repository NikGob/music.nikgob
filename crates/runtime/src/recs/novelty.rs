//! How new music is received, in the runtime: every start
//! of a new song of a service (not the library's, picked by a mix, a radio,
//! autoplay or «Новое для тебя») and how it ended go into the
//! [`NewLog`] the library's cache keeps (`recs.new_log`), with where it was
//! found and its analysed preview. A new song played to the end twice is
//! offered once to be kept: ♡ «Оставить у себя», a like, which downloads
//! it. Its like is the «рекомендованный лайк» the log keeps too. The skip
//! model learns from the starts of new songs as of the library's, with its
//! «new from a service» feature. Each start is kept with the taste profile
//! it was heard for (its preview's of its kind of song) and whether the
//! session was background; a log of before the profiles is keyed once from
//! its previews ([`keyed`]).

use nikgob_core::model::{EventContext, EventOrigin, LikeTarget, TrackId};
use nikgob_library::now_ms;
use nikgob_recs::{Ended, Heard, NewEvent, NewLog, Source, Vector, song_key};
use nikgob_recs::songtype::{SongType, Voice};
use nikgob_view::Command;

use super::recs_now;
use crate::Worker;

const LOG_KEY: &str = "recs.new_log";

/// The log as the library keeps it, its artists keyed as now
/// ([`normalised`]).
pub(super) fn log_of(lib: &nikgob_library::Library) -> NewLog {
    normalised(lib, super::cache_read::<NewLog>(lib, LOG_KEY).unwrap_or_default())
}

/// `log` with each event's artist keyed as the log keys it now
/// ([`nikgob_recs::main_artist_key`] of its song's credit in the library;
/// of the key it has when the song is gone): a log kept before keyed some
/// credits whole («a b» of «A / B»), and its early skips count with the
/// main artist's since. Read time only: the kept log is not rewritten for
/// it (the next event's save keeps it as it reads).
pub(super) fn normalised(lib: &nikgob_library::Library, mut log: NewLog) -> NewLog {
    let ids: Vec<TrackId> = log.events.iter().filter_map(|e| e.track).collect();
    let rows = if ids.is_empty() { Default::default() } else { lib.get_track_rows(&ids).unwrap_or_default() };
    for e in &mut log.events {
        let main = match e.track.and_then(|t| rows.get(&t)) {
            Some(row) => nikgob_recs::main_artist_key(&row.artist),
            None => nikgob_recs::main_artist_key(&e.artist),
        };
        if !main.is_empty() {
            e.artist = main;
        }
    }
    log
}

/// The key of the taste profile a new song sounding `v` (embedded) of
/// `lang` falls in, its best membership.
fn profile_key(snap: &super::Snapshot, v: Option<&Vector>, lang: Option<nikgob_recs::Lang>) -> Option<u32> {
    let kind = match lang {
        Some(l) => SongType { voice: Voice::Sung, lang: Some(l), ..SongType::default() },
        None => SongType::default(),
    };
    let m = snap.model.profiles.membership(v?, &kind);
    let j = (0..m.len()).max_by(|a, b| m[*a].total_cmp(&m[*b]).then(b.cmp(a)))?;
    snap.model.profiles.list.get(j).map(|p| p.key)
}

/// `log` keyed by the taste profiles of `snap` (see the module docs).
pub(super) fn keyed(mut log: NewLog, snap: &super::Snapshot) -> NewLog {
    log.upgrade(|e| profile_key(snap, e.vector.as_ref(), None));
    log
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
    profile: Option<u32>,
}

#[derive(Default)]
pub(crate) struct NewState {
    loaded: bool,
    pub log: NewLog,
    playing: Option<Playing>,
    /// Counts the artists kept out since the start: an open pool built
    /// before a new one is built again ([`super::wave::WavePool`]).
    pub bans: u64,
}

impl Worker {
    pub(crate) fn new_log_load(&mut self) {
        if self.recs.newlog.loaded {
            return;
        }
        self.recs.newlog.loaded = true;
        self.recs.newlog.log = log_of(&self.lib);
        if let Some(snap) = self.snapshot()
            && self.recs.newlog.log.version < nikgob_recs::novelty::NOVELTY_VERSION
        {
            self.recs.newlog.log = keyed(std::mem::take(&mut self.recs.newlog.log), &snap);
            self.new_log_save();
        }
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
        let vector = self.analysis.index().read().get(track).map(|v| snap.model.embed(v));
        let lang = snap.types.lang_of_new(&row.title, &[row.artist.as_str()]);
        Some(Playing {
            track,
            artist: nikgob_recs::main_artist_key(&row.artist),
            title: row.title.clone(),
            source: self.discovery_source(&song),
            profile: profile_key(&snap, vector.as_ref(), lang),
            vector,
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
        let now = recs_now();
        let was_out = self.recs.newlog.log.artist_penalty(&p.artist, now).is_infinite();
        self.recs.newlog.log.push(NewEvent {
            song: p.song.clone(),
            artist: p.artist.clone(),
            source: p.source,
            vector: p.vector,
            track: Some(p.track),
            at: recs_now(),
            heard,
            profile: p.profile,
            background: self.snapshot().is_some_and(|s| s.model.live(&self.recs.session).p_bg >= 0.5),
        });
        if !was_out && self.recs.newlog.log.artist_penalty(&p.artist, now).is_infinite() {
            log::info!("new music: {} is kept out now", p.artist);
            self.recs.newlog.bans += 1;
        }
        self.new_log_save();
    }
}
