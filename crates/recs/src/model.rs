//! The user model: what the history says about every track,
//! world and pair of tracks, a model of the next start and a model of the
//! early skip, both learning online.
//!
//! * [`TrackStats`]: per track two sums of outcome weights, halving in 45
//!   days and in 2 days, counts of starts and outcomes, the last
//!   start and complete, starts by part of the day. An outcome weighs:
//!   complete +1, late skip +0.15 × the share heard, early skip
//!   −0.8 (×0.6 after the user's own choice, ×1.3 after the app's, ×0.5
//!   from the third running in a playlist or the library), a start never
//!   ended +0.1 (0 when the app closed on it), a repeat within a day of a
//!   complete +0.7; positives ×1.2 when the user chose; a like +2.
//! * [`WorldStats`] and the transitions world → world inside sessions;
//!   [`CoOccur`]: track → track inside sessions, `0.5^(k−1)` for the k-th
//!   start after (k ≤ 3), the top [`CO_TOP`] of each track kept.
//! * [`Model`]: a conditional logit over [`FEATURES`] of every candidate
//!   (`scripts/replay.py`, P2): one step after each start the user chose
//!   (`w ← w + η(f(target) − E_P[f]) − ηλ(w − w_prior)`, the expectation
//!   over the 100 best candidates), from the hand weights [`HAND`], the
//!   step halved for the first [`FULL_STEP_AFTER`] starts. Starts the app
//!   chose never teach it (the model would learn its own picks). A build
//!   learning the weights anew steps on the latest [`TEACH_LAST`] starts
//!   only (a history of 135 000 events took over two minutes; the oldest
//!   steps fade under the later ones anyway). Beside the album, the world,
//!   the sound and the pairs, it sees what was listened together with the
//!   session's tracks in the sessions before ([`F_SESS_CO`]), the track
//!   playing now again ([`F_IS_PREV`]: some play one song on repeat) and
//!   another album's first track ([`F_ALBUM_FIRST`]: an album after an
//!   album), each weighed by what the user's own choices teach.
//! * [`SkipModel`]: online logistic regression of an early skip on
//!   [`SKIP_FEATURES`], a step on every start that completed or was skipped
//!   early, whoever chose it.
//!
//! * Once the history is walked ([`UserModel::settle`]): how the user
//!   listens ([`crate::listening`]: the background sessions, if any), the
//!   taste profiles ([`crate::profiles`]) by the plays' outcomes, a
//!   background play counting [`crate::listening::W_BG`], their shares of
//!   the recent plays, the regions of sound they make ([`crate::regions`])
//!   and how much lower each track stands for the dislikes near it
//!   ([`crate::dislike`]).
//! * Imported plays ([`EventOrigin::Import`], another service's history)
//!   teach the long taste alone: the long affinity of their tracks (and
//!   the profiles' mass). Never the session, the pairs, the transitions,
//!   the models or the hot affinity: those are this app's own listening.
//! * A song saved in a service's library and not liked here (an import's
//!   «music library songs») is long taste too: [`R_SAVED`] on its long
//!   affinity and [`MASS_SAVED`] on the profiles' mass, a state like a like.
//!   One the catalog lacks gives its [`MASS_SAVED`] to the catalog's tracks
//!   of its artist, shared among them: the profile of that artist's sound
//!   weighs more.
//!
//! * Who played and where ([`crate::context`]): every outcome counts as
//!   much as the weight of its play's context for the device the model is
//!   built for; the counts of the part of the day and the repeats count a
//!   play of [`crate::context::FULL_PLAY`] at least; the models of the next
//!   start and of the skip never learn from the voice assistant's plays;
//!   the sessions of a speaker and of a computer are walked apart; the
//!   user's background threshold is of the owner's own plays at a
//!   personal device. Then the time habits ([`crate::context::TimeModel`]):
//!   each play by its slot, its profile and its sound.
//!
//! [`UserModel::build`] walks the history in order of time: the stats, and
//! the weights too unless [`Weights`] of the same [`FEATURE_VERSION`] are
//! given (the library's cache keeps them between builds, the runtime
//! teaches them in between). The same walk replays a history prequentially
//! ([`replay`]): every start is predicted before it is learnt.

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, EndReason, EventContext, EventOrigin, EventType, TrackId, UnixMs};

use crate::analysis::Vector;
use crate::context::{FULL_PLAY, PlayActor, PlayDevice, SLOT_HALF_LIFE_MS, TimeModel, TimeSlot, event_weights};
use crate::graph::{GraphInput, SimGraph};
use crate::index::{Normalizer, VectorIndex, euclid};
use crate::listening::{Heard, Listening, SessionSignals, W_BG};
use crate::profiles::{
    ALIVE_DAYS, Input as ProfilesInput, Params, Profiles, SHARE_HALF_DAYS, SharePlay, background_cards, background_shares, quota,
    shares,
};
use crate::regions::Regions;
use crate::session::{Ended, Familiarity, Session, TrackLookup};
use crate::songtype::{SongLane, SongType, SongTypes};
use crate::taste::{Catalog, DAY_MS, DayPart, EARLY_SKIP_MS, Play, SESSION_GAP_MS, Taste, TasteInput};

mod features;
mod history;
mod replay;
mod stats;

pub use replay::*;
pub use stats::*;

#[cfg(test)]
pub(crate) mod tests;

/// What [`UserModel::build`] reads besides the catalog and the taste's input.
#[derive(Clone, Copy)]
pub struct ModelInput<'a> {
    pub index: &'a VectorIndex<TrackId>,
    pub types: Option<&'a SongTypes>,
    /// The weights to go on with (else learnt again from the history).
    pub weights: Option<Weights>,
    /// The collective graph of similar music ([`crate::graph`]), resolved
    /// to the catalog; none: «Похожее по слушателям» is off or nothing is
    /// known yet.
    pub graph: Option<&'a GraphInput>,
}

/// One moment of a session, as the features see it.
#[derive(Clone, Debug)]
pub struct Moment {
    pub now: UnixMs,
    /// The session's tracks by catalog position (unknown ones left out),
    /// oldest first, and how each ended.
    pub sess: Vec<(usize, Ended, EventContext)>,
    pub prev: Option<usize>,
    pub next_album: Option<usize>,
    pub anchor: Option<Vector>,
    /// The sounds the session rejected and how hard each pushes.
    pub pushes: Vec<(Vector, f32)>,
    pub lane: Option<SongLane>,
    pub part: usize,
    pub skip_streak: u8,
    pub album_session: bool,
    pub world_session: Option<u64>,
    pub repeat_one: bool,
    /// How much the session so far is background ([`Listening::p_now`]).
    pub p_bg: f32,
    /// The collective graph's neighbours of the session's last tracks (not
    /// skipped early), by catalog position, and the artists like theirs
    /// (not theirs), each by `0.6^k` of the k-th from the end…
    pub graph_near: HashMap<usize, f32>,
    pub graph_artists: HashMap<ArtistId, f32>,
    /// …and the personal graph's.
    pub pgraph_near: HashMap<usize, f32>,
    /// What was listened in the same sessions as the session's tracks
    /// ([`F_SESS_CO`]), each of those by `0.8^k` of the k-th from the end.
    pub bag_near: HashMap<usize, f32>,
}

/// The rejections of a session measured by the sound index
/// ([`UserModel::push_probe`]).
pub struct PushProbe<'a> {
    index: &'a VectorIndex<TrackId>,
    probe: crate::index::SoundProbe,
    pushed: Vec<(TrackId, f32)>,
}

/// What a live session is ([`UserModel::live`]): one profile or many,
/// how much of it is background, whether it is focused, and the profile of
/// its last start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Live {
    pub mode: Option<crate::listening::Mode>,
    pub p_bg: f32,
    pub focus: bool,
    pub profile: Option<usize>,
}

/// A start of the history as [`UserModel::settle`] reads it.
struct Started {
    ts: UnixMs,
    track: TrackId,
    pos: Option<usize>,
    ended: Ended,
    end: Option<UnixMs>,
    origin: Option<EventOrigin>,
    context: EventContext,
    touches: u32,
    attention: Option<f32>,
    import: bool,
    /// The weight of its context and of its end's ([`crate::context`]).
    weight: f32,
    end_weight: f32,
    /// Not the owner's own play at a personal device: the user's sessions
    /// are told background without it ([`UserModel::listen`]).
    room: bool,
}

/// The home's cards of the taste profiles ([`UserModel::home_profiles`]):
/// how many each one takes, and a background profile's cards as its tracks
/// (catalog positions), the calm first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeProfiles {
    pub cards: Vec<usize>,
    pub background: Vec<Vec<Vec<usize>>>,
}

/// The user model (see the module docs).
#[derive(Clone, Debug)]
pub struct UserModel {
    pub taste: Taste,
    now: UnixMs,
    utc_offset_min: i32,
    /// The local offset of each moment of the history
    /// ([`TasteInput::utc_offsets`]).
    offsets: Vec<(UnixMs, i32)>,
    ids: Vec<TrackId>,
    pos: HashMap<TrackId, usize>,
    pub tracks: Vec<TrackStats>,
    emb: Vec<Option<Vector>>,
    release: Vec<Option<u64>>,
    album_pos: Vec<u32>,
    world: Vec<Option<u64>>,
    artist: Vec<Option<ArtistId>>,
    /// The catalog positions of each main artist.
    by_artist: HashMap<ArtistId, Vec<u32>>,
    duration: Vec<Option<u32>>,
    types: Vec<SongType>,
    next_in_album: Vec<Option<u32>>,
    /// The first track of its album.
    album_first: Vec<bool>,
    pub worlds: HashMap<u64, WorldStats>,
    part_total: [u32; 4],
    trans: HashMap<u64, HashMap<u64, f32>>,
    /// The transitions out of each world in all.
    trans_total: HashMap<u64, f32>,
    /// The catalog positions of each world.
    by_world: HashMap<u64, Vec<u32>>,
    /// Song lane → song lane of the starts the user chose, by
    /// [`lane_index`].
    lane_trans: [[f32; LANES]; LANES],
    pub co: CoOccur,
    /// Tracks listened in the same sessions, either order ([`BAG_WINDOW`]).
    pub bag: CoOccur,
    /// The collective graph and the personal one ([`crate::graph`]).
    pub graph: SimGraph,
    personal: Vec<Vec<(u32, f32)>>,
    /// How cold the history is: 1 with none, `1 / (1 + starts / COLD_STARTS)`,
    /// 0 from [`COLD_UNTIL`] starts.
    cold: f32,
    /// «Без истории — по самой библиотеке».
    library_prior: bool,
    /// The taste profiles ([`crate::profiles`]), the profile of each track,
    /// the profiles' shares of the recent plays (a background play counting
    /// little), the mean `p_bg` of each one's plays and the best long
    /// affinity in each.
    pub profiles: Profiles,
    pub profile_of: Vec<Option<u16>>,
    pub profile_share: Vec<f32>,
    pub profile_bg: Vec<f32>,
    profile_top: Vec<f32>,
    /// How the user listens: the background sessions, if any.
    pub listening: Listening,
    pub weights: Weights,
    /// The median distance between tracks.
    scale: f32,
    norm: Normalizer,
    /// The regions of sound of the taste profiles.
    pub regions: Regions,
    /// When each track was last heard (a play of any origin, a like set by
    /// hand: not one of [`BULK_LIKES`] within an hour) and the latest of
    /// them ([`Self::freshness_at`]).
    last_heard: Vec<Option<UnixMs>>,
    latest_heard: Option<UnixMs>,
    /// Each track's like set by hand (not of a bulk, [`BULK_LIKES`]), and
    /// when the freshness was last brought to the walk's moment.
    hand_likes: Vec<Option<UnixMs>>,
    fresh_at: Option<UnixMs>,
    /// Each track's last imported play the walk has reached.
    imported_at: Vec<Option<UnixMs>>,
    /// The user's own starts that followed another own start of their
    /// session (neither of an album's run, [`EventContext::Album`]), and
    /// those of the same main artist as the one before
    /// ([`Self::artist_again`]).
    again: (u32, u32),
    /// The work of each track whose work is another track
    /// ([`Catalog::work_at`]: another version or copy of one song).
    work: HashMap<u32, TrackId>,
    /// The library's works by their song keys ([`crate::song_key`] of the
    /// work's title and artist, as [`crate::discover::stream_work_key`]
    /// keys a new song) and back ([`Self::set_song_works`]): a new song
    /// and a library track of one song are one work.
    song_works: HashMap<String, TrackId>,
    work_songs: HashMap<TrackId, String>,
    /// When the library's files came (catalog positions of local tracks
    /// added lately, [`ARRIVAL_WINDOW_MS`]): a new arrival never played is
    /// offered among the unheard first ([`Self::arrival_at`]).
    arrived: HashMap<u32, UnixMs>,
    /// How long after a song was heard out it was heard out again (the
    /// user's own plays, not imported ones), by [`REPEAT_BINS`]: how soon
    /// this user likes to hear a song again ([`Self::repeat_rest`]).
    repeat_gaps: [u32; REPEAT_BINS.len() + 1],

    /// The embedded vectors of the disliked tracks (a new track near them
    /// stands lower).
    disliked: Vec<Vector>,
    /// The kind of device it is built for ([`TasteInput::target_device`]).
    device: PlayDevice,
    /// The user's time habits ([`crate::context::TimeModel`]).
    pub time: TimeModel,
    /// The walk: its session, the skip features of open starts, the run of
    /// early skips for leafing, of the kind of device its last event
    /// played on; the other kind's session, run and last event wait
    /// beside it (a speaker's session never runs into the computer's).
    walk: Session,
    pending: HashMap<usize, [f32; G]>,
    streak: u32,
    last_event: Option<UnixMs>,
    walk_device: PlayDevice,
    walk_other: (Session, u32, Option<UnixMs>),
    learn: bool,
    /// The walk teaches the weights from this start on ([`TEACH_LAST`]).
    learn_from: UnixMs,
    /// Imported starts not ended yet, by catalog position.
    import_open: HashMap<usize, UnixMs>,
}

impl TrackLookup for UserModel {
    fn vector(&self, id: TrackId) -> Option<Vector> {
        self.pos.get(&id).and_then(|&i| self.emb[i])
    }
    fn album(&self, id: TrackId) -> Option<(u64, u32)> {
        let &i = self.pos.get(&id)?;
        Some((self.release[i]?, self.album_pos[i]))
    }
    fn world(&self, id: TrackId) -> Option<u64> {
        self.pos.get(&id).and_then(|&i| self.world[i])
    }
    fn lane(&self, id: TrackId) -> Option<SongLane> {
        self.pos.get(&id).map(|&i| self.types[i].lane())
    }
    fn duration(&self, id: TrackId) -> Option<u32> {
        self.pos.get(&id).and_then(|&i| self.duration[i])
    }
    /// New music the catalog lacks is unfamiliar.
    fn familiarity(&self, id: TrackId) -> Familiarity {
        let Some(&i) = self.pos.get(&id) else { return Familiarity::Unfamiliar };
        let t = &self.tracks[i];
        if t.loved(self.now) {
            Familiarity::Loved
        } else if t.unfamiliar(self.now) {
            Familiarity::Unfamiliar
        } else {
            Familiarity::Known
        }
    }
}

/// Whether the models (the next start, the skip) learn from a play of
/// `actor`: the owner's, and an agent's, which plays for the owner; never
/// the voice assistant's or a guest's.
pub(super) fn teaches_by(actor: PlayActor) -> bool {
    matches!(actor, PlayActor::Owner | PlayActor::Agent)
}

/// The places of events in the order the walk takes them: by time, a start
/// after the end it shares a moment with.
pub(super) fn ordered(events: &[Play]) -> Vec<usize> {
    let mut sorted: Vec<usize> = (0..events.len()).collect();
    sorted.sort_by_key(|&k| (events[k].ts, events[k].kind == EventType::Start));
    sorted
}

impl UserModel {
    /// The model of the history of `input` over `catalog` at `now` (see
    /// the module docs); the taste of [`Taste::build`] with it.
    pub fn build(catalog: &Catalog, input: &TasteInput, extra: ModelInput<'_>, now: UnixMs) -> Self {
        Self::build_with(catalog, input, extra, now, None)
    }

    /// [`Self::build`], `hook` seeing every start of the walk that follows
    /// another of its session (see [`Self::event`]).
    fn build_with(catalog: &Catalog, input: &TasteInput, extra: ModelInput<'_>, now: UnixMs, mut hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) -> Self {
        // Each event by who played it where, for the device it is built for.
        let weights = event_weights(&input.events, input.target_device);
        let taste = Taste::build_weighted(catalog, input, &weights, now);
        let mut m = Self::empty(catalog, taste, extra, input.utc_offset_min, now);
        m.device = input.target_device;
        m.offsets = input.utc_offsets.clone();
        m.learn = extra.weights.is_none_or(|w| w.version != FEATURE_VERSION);
        m.learn_from = teach_from(&input.events);
        if let Some(w) = extra.weights.filter(|w| w.version == FEATURE_VERSION) {
            m.weights = w;
        }
        // Likes as they are, from when they were set (else always).
        for id in &input.likes {
            if let Some(&i) = m.pos.get(id) {
                m.tracks[i].liked = Some(i64::MIN);
            }
        }
        for (id, at) in &input.like_times {
            if let Some(&i) = m.pos.get(id)
                && m.tracks[i].liked.is_some()
            {
                m.tracks[i].liked = Some(*at);
            }
        }
        for id in &input.saved {
            if let Some(&i) = m.pos.get(id) {
                m.tracks[i].saved = true;
            }
        }
        if input.personal_graph {
            m.personal = crate::graph::personal(catalog, &input.events, &input.playlists);
        }
        // How the user listens, before the walk: the skip model reads the
        // background of each moment.
        let order = ordered(&input.events);
        let (starts, p_bg) = m.listen(input, &order, &weights);
        m.hand_likes = m.hand_likes_of();
        for &k in &order {
            m.event(&input.events[k], weights[k], hook.as_mut().map(|h| &mut **h as &mut dyn FnMut(&Self, &Moment, usize, &Play)));
        }
        m.close_imported();
        m.now = now;
        m.learn = false;
        m.settle(catalog, input, &starts, &p_bg);
        m
    }

    /// What a live `session` is: pure or mixed by the
    /// profiles of its starts, its `p_bg` ranked among the user's own
    /// sessions, the profile of its last start.
    pub fn live(&self, session: &Session) -> Live {
        let profile = |t: TrackId| self.position(t).and_then(|i| self.profile_at(i));
        let signals = SessionSignals::of_session(session, |t| self.position(t).and_then(|i| self.duration[i]));
        Live {
            mode: session.mode(profile),
            p_bg: self.moment(session, session.last_ts().unwrap_or(self.now)).p_bg,
            focus: self.listening.focused(&signals),
            profile: session.last().and_then(|p| profile(p.track)),
        }
    }

    /// The profile of the track at `i`.
    pub fn profile_at(&self, i: usize) -> Option<usize> {
        self.profile_of.get(i).copied().flatten().map(usize::from)
    }

    /// The soft membership of the track at `i` in each profile
    /// ([`Profiles::membership`]); a track without a sound is wholly of
    /// its own profile.
    pub fn membership_at(&self, i: usize) -> Vec<f32> {
        match &self.emb[i] {
            Some(e) => self.profiles.membership(e, &self.types[i]),
            None => (0..self.profiles.len()).map(|j| if self.profile_at(i) == Some(j) { 1.0 } else { 0.0 }).collect(),
        }
    }

    /// The length of the track at `i`.
    pub fn duration_at(&self, i: usize) -> Option<u32> {
        self.duration[i]
    }

    /// The long affinity of the track at `i` within its profile: 1 for the
    /// best loved of it, so a small profile's favourites stand as high as a
    /// big one's.
    pub fn profile_affinity(&self, i: usize) -> f32 {
        let a = self.tracks[i].aff_long(self.now);
        match self.profile_at(i).and_then(|j| self.profile_top.get(j)) {
            Some(top) if *top > 1e-6 => a / top,
            _ => a,
        }
    }

    /// Whether profile `j` is a background profile: most of its plays
    /// background, of a user who has background sessions at all.
    pub fn is_background_profile(&self, j: usize) -> bool {
        self.listening.has_background && self.profile_bg.get(j).is_some_and(|b| *b >= 0.5)
    }

    /// The home's cards of the profiles out of `slots`. A background profile whose tracks suit the
    /// background (`suits`, see [`crate::background`]) takes up to
    /// `bg_cards` cards by its share, of those tracks only, split by
    /// energy; one with too few (its background is its ordinary songs)
    /// counts as any other. Every active profile keeps a card.
    pub fn home_profiles(&self, slots: usize, bg_cards: usize, suits: impl Fn(usize) -> bool) -> HomeProfiles {
        let k = self.profiles.len();
        let mut suited: Vec<Vec<(usize, f32)>> = vec![Vec::new(); k];
        for i in 0..self.len() {
            if let Some(j) = self.profile_at(i).filter(|&j| self.is_background_profile(j))
                && suits(i)
            {
                let energy = self.emb[i].map_or(0.0, |e| crate::mood::Axes::of(&self.norm.z_of_embedded(&e)).energy);
                suited[j].push((i, energy));
            }
        }
        let room: Vec<usize> = suited.iter().map(|s| background_cards(s, bg_cards).len()).collect();
        let background: Vec<bool> = room.iter().map(|r| *r > 0).collect();
        let cap = room.iter().copied().max().unwrap_or(0).max(1);
        let mut cards = quota(&self.profile_share, slots, &background, cap);
        // Each background profile's own cap.
        for j in 0..k {
            if background[j] && cards[j] > room[j] {
                let spare = cards[j] - room[j];
                cards[j] = room[j];
                if let Some(o) = (0..k).filter(|&o| !background[o] && cards[o] > 0).max_by(|a, b| self.profile_share[*a].total_cmp(&self.profile_share[*b])) {
                    cards[o] += spare;
                }
            }
        }
        let background = (0..k).map(|j| if background[j] { background_cards(&suited[j], cards[j]) } else { Vec::new() }).collect();
        HomeProfiles { cards, background }
    }

    /// Every start of the history (`order` of [`ordered`], `weights` of
    /// its events) and how it ended, its seeks and the likes set while it
    /// played as its touches, in order of time. A guest's are not read.
    fn starts_of(&self, input: &TasteInput, order: &[usize], weights: &[f32]) -> Vec<Started> {
        let mut out: Vec<Started> = Vec::new();
        // By track and kind of device: the same song on a speaker and at
        // the computer at once are two plays (as [`event_weights`] pairs them).
        let mut open: HashMap<(TrackId, PlayDevice), usize> = HashMap::new();
        for &e in order {
            let p = &input.events[e];
            let w = weights.get(e).copied().unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }
            match p.kind {
                EventType::Start => {
                    open.insert((p.track, p.device), out.len());
                    out.push(Started {
                        ts: p.ts,
                        pos: self.pos.get(&p.track).copied(),
                        track: p.track,
                        ended: Ended::Open,
                        end: None,
                        origin: p.origin,
                        context: p.context,
                        touches: 0,
                        attention: None,
                        import: p.origin == Some(EventOrigin::Import),
                        weight: w,
                        end_weight: w,
                        room: p.actor != PlayActor::Owner || p.device != PlayDevice::Personal,
                    });
                }
                EventType::Complete | EventType::Skip => {
                    let Some(k) = open.remove(&(p.track, p.device)) else { continue };
                    let s = &mut out[k];
                    s.end = Some(p.ts);
                    s.end_weight = w;
                    s.attention = p.attention;
                    s.ended = match p.kind {
                        EventType::Complete => Ended::Complete,
                        _ if p.end_reason == Some(EndReason::AppClosed) => Ended::Open,
                        _ if p.position_ms.is_some_and(|x| x < EARLY_SKIP_MS) => Ended::Early,
                        _ => Ended::Late,
                    };
                }
                EventType::Seek => {
                    if let Some(&k) = open.get(&(p.track, p.device)) {
                        out[k].touches += 1;
                    }
                }
                _ => {}
            }
        }
        let mut by_track: HashMap<TrackId, Vec<usize>> = HashMap::new();
        for (k, s) in out.iter().enumerate().filter(|(_, s)| !s.import) {
            by_track.entry(s.track).or_default().push(k);
        }
        for (t, at) in &input.like_times {
            let Some(list) = by_track.get(t) else { continue };
            let k = list.partition_point(|&k| out[k].ts <= *at);
            if let Some(&k) = k.checked_sub(1).map(|x| &list[x])
                && *at - out[k].ts <= LIKE_IN_PLAY_MS
            {
                out[k].touches += 1;
            }
        }
        out
    }

    /// How the user listens ([`Listening`]): of the user's own starts of
    /// `input` at a personal device (the threshold of background is the
    /// user's at the screen: a speaker's runs, the assistant's and an
    /// agent's plays are weighed by their context instead, `p_bg` 0); the
    /// starts and the `p_bg` of each.
    fn listen(&mut self, input: &TasteInput, order: &[usize], weights: &[f32]) -> (Vec<Started>, Vec<f32>) {
        let starts = self.starts_of(input, order, weights);
        let own: Vec<usize> = (0..starts.len()).filter(|&k| !starts[k].import && !starts[k].room).collect();
        let heard: Vec<Heard> = own
            .iter()
            .enumerate()
            .map(|(x, &k)| {
                let s = &starts[k];
                // A history older than origins: a start right after one not
                // heard out, not of a radio, is the user's pick.
                let manual = match s.origin {
                    Some(o) => o == EventOrigin::Manual,
                    None => x > 0 && {
                        let prev = &starts[own[x - 1]];
                        s.ts - prev.ts < SESSION_GAP_MS && prev.ended != Ended::Complete && s.context != EventContext::Radio
                    },
                };
                Heard {
                    ts: s.ts,
                    end: s.end,
                    ended: s.ended,
                    manual,
                    touches: s.touches,
                    attention: s.attention,
                    duration_ms: s.pos.and_then(|i| self.duration[i]),
                }
            })
            .collect();
        let (listening, p_own) = Listening::of(&heard);
        let mut p_bg = vec![0.0f32; starts.len()];
        for (x, &k) in own.iter().enumerate() {
            p_bg[k] = p_own[x];
        }
        self.listening = listening;
        (starts, p_bg)
    }

    /// The taste profiles, their shares and the dislikes' neighbours, once
    /// the history is walked (`starts` and their `p_bg` of [`Self::listen`]).
    fn settle(&mut self, catalog: &Catalog, input: &TasteInput, starts: &[Started], p_bg: &[f32]) {
        let now = self.now;
        let n = self.len();
        self.settle_freshness();
        let set = &input.profiles;
        let w_bg = if set.background_less { W_BG } else { 1.0 };
        let own: Vec<usize> = (0..starts.len()).filter(|&k| !starts[k].import).collect();
        // The profiles' mass: the outcomes, a background play counting
        // little; likes and playlists. And their life: the active plays of
        // the last two months.
        let mut mass = vec![0.0f32; n];
        let mut alive = vec![0.0f32; n];
        let mut plays: Vec<(UnixMs, usize)> = Vec::new();
        // The history's plays as much as each weighs (the cold start's
        // measure: a speaker's marathon does not end it).
        let mut heard_mass = 0.0f32;
        for (k, s) in starts.iter().enumerate() {
            heard_mass += s.weight;
            let Some(i) = s.pos else { continue };
            let r = match s.ended {
                Ended::Complete => MASS_COMPLETE,
                Ended::Late => MASS_LATE,
                Ended::Open => MASS_OPEN,
                Ended::Early => MASS_EARLY,
            };
            // As much as its context weighs (an early skip as its skip does).
            let w = if s.ended == Ended::Early { s.end_weight } else { s.weight };
            mass[i] += r * halve(1.0, s.ts, now, H_MASS_MS) * (1.0 - (1.0 - w_bg) * p_bg[k]) * w;
            if p_bg[k] < 0.5 && s.weight >= FULL_PLAY && s.ended != Ended::Early && now - s.ts <= ALIVE_DAYS * DAY_MS {
                alive[i] += 1.0;
            }
            // The plays the profiles are cut from: whole ones.
            if s.weight >= FULL_PLAY {
                plays.push((s.ts, i));
            }
        }
        for (i, m) in mass.iter_mut().enumerate() {
            if self.tracks[i].liked(now) {
                *m += MASS_LIKE;
            } else if self.tracks[i].saved {
                *m += MASS_SAVED;
            }
        }
        for (t, added) in &input.playlist_entries {
            if let Some(&i) = self.pos.get(t) {
                mass[i] += MASS_PLAYLIST * halve(1.0, *added, now, H_MASS_MS);
            }
        }
        for (_, artists) in self.taste.saved_elsewhere() {
            let Some(of) = artists.iter().map(|a| catalog.of_artist(*a)).find(|of| !of.is_empty()) else { continue };
            for &i in of {
                mass[i as usize] += MASS_SAVED / of.len() as f32;
            }
        }
        // The library is a taste too while the history is short: every
        // track a little, the ones the graph gathers around more.
        self.cold = if heard_mass >= COLD_UNTIL as f32 { 0.0 } else { 1.0 / (1.0 + heard_mass / COLD_STARTS) };
        self.library_prior = input.library_prior;
        for (i, m) in mass.iter_mut().enumerate() {
            *m += self.prior_mass(i);
        }
        for (i, m) in mass.iter_mut().enumerate() {
            if self.taste.blocks(&catalog.tracks()[i]) {
                *m = 0.0;
            }
        }
        let playlists: Vec<Vec<usize>> = input.playlists.iter().map(|l| l.iter().filter_map(|t| self.pos.get(t).copied()).collect()).collect();
        let day = (now + i64::from(self.utc_offset_min) * 60_000).div_euclid(DAY_MS);
        self.profiles = Profiles::build(&ProfilesInput {
            ids: &self.ids,
            sound: &self.emb,
            types: &self.types,
            artists: &self.artist,
            mass: &mass,
            alive: &alive,
            plays: &plays,
            playlists: &playlists,
            scale: self.scale,
            params: Params { split: set.split_by_sound, ..Params::default() },
            history: set.history.as_ref(),
            day,
        });
        self.regions = self.profiles.regions(&self.emb);
        self.profile_of = self.profiles.all_of().to_vec();
        let k = self.profiles.len();
        // Their shares of the recent plays (an early skip is no choice of it)
        // and how much of each one's plays is background.
        // A play imported from another service is the long taste: beside
        // the user's own plays here it counts [`IMPORT_SHARE`] (a fresh
        // import of a whole history would take the shares over as if it
        // were today's listening); alone, it is all there is.
        let own_listening = own.iter().any(|&k| starts[k].pos.is_some() && starts[k].ended != Ended::Early);
        let share_play = |x: usize| {
            let s = &starts[x];
            let i = s.pos?;
            let weight = if s.ended == Ended::Early {
                0.0
            } else if s.import && own_listening {
                IMPORT_SHARE
            } else {
                s.weight
            };
            Some(SharePlay { ts: s.ts, profile: self.profile_at(i), p_bg: p_bg[x], weight })
        };
        let all: Vec<SharePlay> = (0..starts.len()).filter_map(share_play).collect();
        // How much of each profile is background: of the user's own plays
        // at a personal device (the rest is weighed by its context).
        let own_plays: Vec<SharePlay> = own.iter().filter(|&&x| !starts[x].room).filter_map(|&x| share_play(x)).collect();
        let sh = shares(&all, k, w_bg, Some(SHARE_HALF_DAYS), now);
        // No plays yet: the profiles' shares of the mass (the likes, the
        // playlists, the library's prior).
        let mut by_mass = vec![0.0f32; k];
        for (i, m) in mass.iter().enumerate() {
            if let Some(j) = self.profile_at(i) {
                by_mass[j] += m.max(0.0);
            }
        }
        let total_mass: f32 = by_mass.iter().sum();
        self.profile_share = if sh.iter().sum::<f32>() > 0.0 {
            sh
        } else if total_mass > 0.0 {
            by_mass.iter().map(|m| m / total_mass).collect()
        } else {
            vec![1.0 / k.max(1) as f32; k]
        };
        self.profile_bg = background_shares(&own_plays, k);
        let mut top = vec![0.0f32; k];
        for i in 0..n {
            if let Some(j) = self.profile_at(i) {
                top[j] = top[j].max(self.tracks[i].aff_long(now));
            }
        }
        self.profile_top = top;
        // The time habits: each play heard (not skipped early) by its
        // outcome, its context and its age, in its slot, of its profile,
        // with its sound; an imported one a little.
        let mut time = TimeModel::new(k);
        for s in starts {
            let Some(i) = s.pos else { continue };
            let outcome = match s.ended {
                Ended::Complete => MASS_COMPLETE,
                Ended::Late => MASS_LATE,
                Ended::Open => MASS_OPEN,
                Ended::Early => continue,
            };
            let w = if s.import { IMPORT_SHARE } else { s.weight };
            let m = outcome * w * halve(1.0, s.ts, now, SLOT_HALF_LIFE_MS);
            // Its slot by its own day's clock.
            time.add(TimeSlot::at(s.ts, input.offset_at(s.ts)), self.profile_at(i), m, self.sound_point_at(i));
        }
        self.time = time;
        let disliked: Vec<usize> = input.disliked_tracks.iter().filter_map(|t| self.pos.get(t).copied()).collect();
        if input.dislike_neighbours {
            self.disliked = disliked.iter().filter_map(|&i| self.emb[i]).collect();
            let near = crate::dislike::near_dislikes(&self.emb, &disliked, self.scale);
            self.taste.set_near_dislikes(near);
        }
    }

    /// When each track was last heard and how much of its like counts now
    /// (see [`Self::freshness_at`]): every surface weighs an old love and a
    /// bulk of likes so.
    fn settle_freshness(&mut self) {
        self.hand_likes = self.hand_likes_of();
        self.fresh_at = Some(self.now);
        self.freshness_as_of(UnixMs::MAX, false);
    }

    /// Each track's like set by hand: not one of [`BULK_LIKES`] within
    /// [`BULK_WINDOW_MS`] (a service's library brought at once).
    fn hand_likes_of(&self) -> Vec<Option<UnixMs>> {
        let n = self.len();
        let mut likes: Vec<UnixMs> = (0..n).filter_map(|i| self.taste.liked_time_at(i)).collect();
        likes.sort_unstable();
        let by_hand = |t: UnixMs| {
            let from = likes.partition_point(|x| *x < t - BULK_WINDOW_MS);
            let to = likes.partition_point(|x| *x <= t + BULK_WINDOW_MS);
            to - from < BULK_LIKES
        };
        (0..n).map(|i| self.taste.liked_time_at(i).filter(|t| by_hand(*t))).collect()
    }

    /// The freshness of every like as it was at `at` (see
    /// [`Self::freshness_at`]): what was heard by then — the walk's own
    /// starts and completes, the last play when it was before `at`, a like
    /// set by hand by then, an imported play the walk passed; `at` itself
    /// when the user is `listening` then
    /// (a start). The walk of the history brings it to each day it reaches
    /// (and to each session after a pause), so a like learns as it counted
    /// then, not as it counts at the end.
    fn freshness_as_of(&mut self, at: UnixMs, listening: bool) {
        let n = self.len();
        self.last_heard = (0..n)
            .map(|i| {
                let s = &self.tracks[i];
                let played = self.taste.last_played_at(i).filter(|t| *t <= at);
                let liked = self.hand_likes.get(i).copied().flatten().filter(|t| *t <= at);
                let imported = self.imported_at.get(i).copied().flatten();
                [played, s.last_start, s.last_complete, liked, imported].into_iter().flatten().max()
            })
            .collect();
        self.latest_heard = self.last_heard.iter().flatten().copied().chain(listening.then_some(at)).max();
        for i in 0..n {
            let stale = 1.0 - self.freshness_at(i);
            self.tracks[i].love_stale = stale;
        }
    }

    /// When the track at `i` was last heard (see [`Self::last_heard`]).
    pub fn last_heard_at(&self, i: usize) -> Option<UnixMs> {
        self.last_heard.get(i).copied().flatten()
    }

    /// The user's latest listening (a play, a like set by hand).
    pub fn latest_heard(&self) -> Option<UnixMs> {
        self.latest_heard
    }

    /// How much of a love of the track at `i` counts now: whole while it
    /// was heard within [`FRESH_GRACE_MS`] of the user's latest listening
    /// (a user back after a year has last year's weeks, not none), then by
    /// half every [`FRESH_HALF_MS`] down to [`STALE_FLOOR`]; never heard
    /// once anything was, [`UNHEARD_LOVE`].
    pub fn freshness_at(&self, i: usize) -> f32 {
        match (self.latest_heard, self.last_heard_at(i)) {
            (Some(l), Some(h)) => freshness(l - h),
            (Some(_), None) => UNHEARD_LOVE,
            _ => 1.0,
        }
    }

    /// The share of the user's own next starts after an own start (out of
    /// an album) that were of the same artist as the one before, shrunk to
    /// `prior` by ten
    /// starts: how much an artist again is the user's own way of
    /// listening.
    pub fn artist_again(&self, prior: f32) -> f32 {
        (self.again.1 as f32 + 10.0 * prior) / (self.again.0 as f32 + 10.0)
    }

    /// The work of the track at `i`: the track standing for its song when
    /// it is another version or copy of a song ([`Catalog::work_at`]), else
    /// itself.
    pub fn work_at(&self, i: usize) -> TrackId {
        self.work.get(&(i as u32)).copied().unwrap_or(self.ids[i])
    }

    /// The song keys of the library's works (`(key, track)`: the key of a
    /// track's work title and artist, the work [`Self::work_at`] of the
    /// track), so a new song of one of them is that work; the first of a
    /// key, and the first key of a work, win.
    pub fn set_song_works(&mut self, keys: impl IntoIterator<Item = (String, TrackId)>) {
        for (key, track) in keys {
            let Some(i) = self.position(track) else { continue };
            let work = self.work_at(i);
            self.work_songs.entry(work).or_insert_with(|| key.clone());
            self.song_works.entry(key).or_insert(work);
        }
    }

    /// How much the library's song at `i` is a new arrival: a file come
    /// lately and never played, 1 just added, half after
    /// [`ARRIVAL_HALF_MS`], none past [`ARRIVAL_WINDOW_MS`] or once played.
    pub fn arrival_at(&self, i: usize) -> f32 {
        let Some(&at) = self.arrived.get(&(i as u32)) else { return 0.0 };
        if self.tracks[i].starts > 0 {
            return 0.0;
        }
        let age = self.now - at;
        if !(0..=ARRIVAL_WINDOW_MS).contains(&age) {
            return 0.0;
        }
        0.5f32.powf(age as f32 / ARRIVAL_HALF_MS as f32)
    }

    /// The library work a new song's work key is of ([`Self::set_song_works`]).
    pub fn work_of_song(&self, key: &str) -> Option<TrackId> {
        self.song_works.get(key).copied()
    }

    /// The song key of the library's work `work` ([`Self::set_song_works`]).
    pub fn song_of_work(&self, work: TrackId) -> Option<&str> {
        self.work_songs.get(&work).map(String::as_str)
    }

    /// How long a song heard out by this user rests as a rule: the median
    /// time after which their songs heard out were heard out again (the
    /// upper bound of its bin), [`REPEAT_DEFAULT_MS`] with fewer than
    /// [`REPEAT_MIN`] repeats known.
    pub fn repeat_rest(&self) -> i64 {
        let n: u32 = self.repeat_gaps.iter().sum();
        if n < REPEAT_MIN {
            return REPEAT_DEFAULT_MS;
        }
        let mut seen = 0;
        for (k, c) in self.repeat_gaps.iter().enumerate() {
            seen += c;
            if 2 * seen >= n {
                return REPEAT_BINS.get(k).copied().unwrap_or(2 * REPEAT_BINS[REPEAT_BINS.len() - 1]);
            }
        }
        REPEAT_DEFAULT_MS
    }

    /// The chance of an early skip of the track at `i`, `p` by the model:
    /// at least what its last outcomes say when it is worn out now
    /// ([`TrackStats::burnt`]).
    pub fn skip_risk(&self, i: usize, p: f32) -> f32 {
        self.tracks.get(i).and_then(|t| t.burnt(p)).map_or(p, |b| b.max(p))
    }

    /// How worn out the track at `i` is now, as a chance of an early skip:
    /// what its last outcomes say when most were early
    /// ([`TrackStats::burnt`], shrunk to none), 0 else — however it was
    /// skipped before, a song heard out again lately is not worn. The
    /// wave's own pull reads it as a weight against the song (by the model,
    /// [`Self::skip_risk`] holds it).
    pub fn worn(&self, i: usize) -> f32 {
        self.tracks.get(i).and_then(|t| t.burnt(0.0)).unwrap_or(0.0)
    }

    /// The graph's prior of the track at `i` (how central it is, how
    /// popular) as a cold start: whole for a track never started, fading
    /// with its own starts — what the user plays of it says more.
    pub fn prior_at(&self, i: usize) -> f32 {
        self.graph.prior(i) / (1.0 + self.tracks.get(i).map_or(0, |t| t.starts) as f32)
    }

    /// A raw bliss vector in the model's space.
    pub fn embed(&self, v: &Vector) -> Vector {
        self.norm.embed(v)
    }

    /// How much lower a new track sounding `v` (embedded) stands for the
    /// dislikes near it.
    pub fn dislike_near_of(&self, v: &Vector) -> f32 {
        crate::dislike::near_dislikes_of(v, &self.disliked, self.scale)
    }

    pub(super) fn empty(catalog: &Catalog, taste: Taste, extra: ModelInput<'_>, utc_offset_min: i32, now: UnixMs) -> Self {
        let n = catalog.len();
        let norm = extra.index.normalizer();
        let ids: Vec<TrackId> = catalog.tracks().iter().map(|t| t.id).collect();
        let mut pos: HashMap<TrackId, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        // A copy of a catalog track (one recording on two albums) is it.
        for (id, i) in catalog.aliases() {
            pos.entry(id).or_insert(i);
        }
        let emb: Vec<Option<Vector>> = ids.iter().map(|id| extra.index.get(*id).map(|v| norm.embed(v))).collect();
        let release: Vec<Option<u64>> = catalog.tracks().iter().map(|t| t.release.map(|r| crate::pick::token(r.as_uuid().as_bytes()))).collect();
        let album_pos: Vec<u32> = catalog.tracks().iter().map(|t| t.position).collect();
        let mut by_release: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, r) in release.iter().enumerate() {
            if let Some(r) = r {
                by_release.entry(*r).or_default().push(i);
            }
        }
        let mut next_in_album = vec![None; n];
        for list in by_release.values_mut() {
            list.sort_by_key(|&i| (album_pos[i], i));
            for w in list.windows(2) {
                if album_pos[w[1]] > album_pos[w[0]] {
                    next_in_album[w[0]] = Some(w[1] as u32);
                }
            }
            // Equal places: the next greater one.
            for k in 0..list.len() {
                if next_in_album[list[k]].is_none() {
                    next_in_album[list[k]] = list[k + 1..].iter().find(|&&j| album_pos[j] > album_pos[list[k]]).map(|&j| j as u32);
                }
            }
        }
        let mut album_first = vec![false; n];
        for list in by_release.values() {
            if let Some(&i) = list.first() {
                album_first[i] = true;
            }
        }
        let types = ids.iter().map(|id| extra.types.and_then(|t| t.get(*id)).copied().unwrap_or_default()).collect();
        let work: HashMap<u32, TrackId> = (0..n).filter(|&i| catalog.work_at(i) != ids[i]).map(|i| (i as u32, catalog.work_at(i))).collect();
        // A day that brought more than a batch is the library's import, not
        // new arrivals.
        let mut per_day: HashMap<i64, usize> = HashMap::new();
        for t in catalog.tracks().iter().filter(|t| t.local && t.added_at > 0 && t.added_at <= now) {
            *per_day.entry(t.added_at.div_euclid(DAY_MS)).or_default() += 1;
        }
        let arrived: HashMap<u32, UnixMs> = catalog
            .tracks()
            .iter()
            .enumerate()
            .filter(|(_, t)| t.local && t.added_at > 0 && t.added_at <= now && now - t.added_at <= ARRIVAL_WINDOW_MS)
            .filter(|(_, t)| per_day.get(&t.added_at.div_euclid(DAY_MS)).is_some_and(|&n| n <= ARRIVAL_BATCH))
            .map(|(i, t)| (i as u32, t.added_at))
            .collect();
        let vs: Vec<&Vector> = emb.iter().flatten().collect();
        let scale = {
            let step = (vs.len() / 64).max(1);
            let sample: Vec<&&Vector> = vs.iter().step_by(step).collect();
            let half = sample.len() / 2;
            let mut d: Vec<f32> = (0..half).map(|k| euclid(*sample[k], *sample[k + half])).collect();
            d.sort_by(f32::total_cmp);
            d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(2.0)
        };
        Self {
            taste,
            now,
            utc_offset_min,
            offsets: Vec::new(),
            ids,
            pos,
            tracks: vec![TrackStats::default(); n],
            emb,
            release,
            album_pos,
            world: catalog.tracks().iter().map(|t| t.world).collect(),
            artist: catalog.tracks().iter().map(|t| t.main_artist()).collect(),
            by_artist: catalog.tracks().iter().enumerate().fold(HashMap::new(), |mut m: HashMap<ArtistId, Vec<u32>>, (i, t)| {
                if let Some(a) = t.main_artist() {
                    m.entry(a).or_default().push(i as u32);
                }
                m
            }),
            duration: catalog.tracks().iter().map(|t| t.duration_ms).collect(),
            types,
            next_in_album,
            album_first,
            worlds: HashMap::new(),
            part_total: [0; 4],
            trans: HashMap::new(),
            trans_total: HashMap::new(),
            by_world: catalog.tracks().iter().enumerate().fold(HashMap::new(), |mut m: HashMap<u64, Vec<u32>>, (i, t)| {
                if let Some(w) = t.world {
                    m.entry(w).or_default().push(i as u32);
                }
                m
            }),
            lane_trans: [[0.0; LANES]; LANES],
            co: CoOccur::new(n),
            bag: CoOccur::new(n),
            graph: extra.graph.filter(|g| !g.is_empty()).map(|g| SimGraph::build(catalog, g)).unwrap_or_default(),
            personal: Vec::new(),
            cold: 1.0,
            library_prior: false,
            profiles: Profiles::default(),
            profile_of: vec![None; n],
            profile_share: Vec::new(),
            profile_bg: Vec::new(),
            profile_top: Vec::new(),
            listening: Listening::default(),
            weights: Weights::default(),
            scale,
            norm,
            regions: Regions::default(),
            last_heard: Vec::new(),
            latest_heard: None,
            hand_likes: Vec::new(),
            fresh_at: None,
            imported_at: Vec::new(),
            again: (0, 0),
            work,
            song_works: HashMap::new(),
            work_songs: HashMap::new(),
            arrived,
            repeat_gaps: [0; REPEAT_BINS.len() + 1],
            disliked: Vec::new(),
            device: PlayDevice::Personal,
            time: TimeModel::default(),
            walk: Session::default(),
            pending: HashMap::new(),
            streak: 0,
            last_event: None,
            walk_device: PlayDevice::Personal,
            walk_other: (Session::default(), 0, None),
            learn: false,
            learn_from: UnixMs::MIN,
            import_open: HashMap::new(),
        }
    }

    pub fn now(&self) -> UnixMs {
        self.now
    }

    /// How cold the history is: 1 with none, towards 0 as it grows.
    pub fn cold(&self) -> f32 {
        self.cold
    }

    /// What the library's prior gives the track at `i` while the history
    /// is short (the taste profiles' mass, a mix's love of it): up to
    /// [`COLD_MASS`], fading with the history; 0 when it is off.
    pub fn prior_mass(&self, i: usize) -> f32 {
        if self.library_prior { COLD_MASS * self.cold * (0.3 + 0.7 * self.graph.prior(i)) } else { 0.0 }
    }

    /// The personal graph's neighbours of the track at `i`.
    pub fn personal_near(&self, i: usize) -> &[(u32, f32)] {
        self.personal.get(i).map_or(&[], Vec::as_slice)
    }

    /// The tracks of artist `a` (main artist), by catalog position.
    pub fn of_artist(&self, a: ArtistId) -> &[u32] {
        self.by_artist.get(&a).map_or(&[], Vec::as_slice)
    }

    pub fn position(&self, id: TrackId) -> Option<usize> {
        self.pos.get(&id).copied()
    }

    pub fn id(&self, i: usize) -> TrackId {
        self.ids[i]
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn embedded_at(&self, i: usize) -> Option<&Vector> {
        self.emb[i].as_ref()
    }

    pub fn song_type_at(&self, i: usize) -> &SongType {
        &self.types[i]
    }

    pub fn world_at(&self, i: usize) -> Option<u64> {
        self.world[i]
    }

    pub fn main_artist_at(&self, i: usize) -> Option<ArtistId> {
        self.artist[i]
    }

    /// Songs of lane `b` may follow songs of lane `a` in a radio of `a`:
    /// the same kind, anything unknown, or a kind this user goes on to
    /// from `a` in at least [`LANE_FOLLOWS`] of their own choices (once
    /// they made [`LANE_EVIDENCE`] of them).
    pub fn lanes_go(&self, a: SongLane, b: SongLane) -> bool {
        if a == b || a == SongLane::Unknown || b == SongLane::Unknown {
            return true;
        }
        let row = &self.lane_trans[lane_index(a)];
        let total: f32 = row.iter().sum();
        total >= LANE_EVIDENCE && row[lane_index(b)] / total >= LANE_FOLLOWS
    }

    pub fn release_at(&self, i: usize) -> Option<u64> {
        self.release[i]
    }

    pub fn next_in_album(&self, i: usize) -> Option<usize> {
        self.next_in_album[i].map(|j| j as usize)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The energy of the track at `i` on the mood axes ([`crate::mood::Axes`]),
    /// when it has a sound.
    pub fn energy_at(&self, i: usize) -> Option<f32> {
        self.emb[i].map(|e| self.energy_of(&e))
    }

    /// The energy of an embedded sound `v` on the mood axes.
    pub fn energy_of(&self, v: &Vector) -> f32 {
        crate::mood::Axes::of(&self.norm.z_of_embedded(v)).energy
    }

    /// The loudness of the track at `i` as bliss gives it (absolute, −1
    /// silence … 1 full scale), when it has a sound.
    pub fn loudness_at(&self, i: usize) -> Option<f32> {
        self.emb[i].and_then(|e| self.norm.raw_of_embedded(&e, crate::mood::LOUDNESS))
    }

    /// The track at `i` on the mood axes, when it has a sound.
    pub fn axes_at(&self, i: usize) -> Option<crate::mood::Axes> {
        self.emb[i].map(|e| self.axes_of(&e))
    }

    /// An embedded sound `v` on the mood axes.
    pub fn axes_of(&self, v: &Vector) -> crate::mood::Axes {
        crate::mood::Axes::of(&self.norm.z_of_embedded(v))
    }

    /// The sound of the track at `i` as the time habits read it: energy,
    /// tempo and steadiness (z over the library), when it has a sound.
    pub fn sound_point_at(&self, i: usize) -> Option<[f32; 3]> {
        self.emb.get(i).copied().flatten().map(|e| self.sound_point_of(&e))
    }

    /// [`Self::sound_point_at`] of an embedded sound `v` (a new song's
    /// preview).
    pub fn sound_point_of(&self, v: &Vector) -> [f32; 3] {
        let z = self.norm.z_of_embedded(v);
        let a = crate::mood::Axes::of(&z);
        [a.energy, z[crate::mood::TEMPO], a.steadiness]
    }

    /// The kind of device it is built for.
    pub fn device(&self) -> PlayDevice {
        self.device
    }

    /// The part of the day of the week at [`Self::now`].
    pub fn slot(&self) -> TimeSlot {
        TimeSlot::at(self.now, self.utc_offset_min)
    }

    /// How much the track at `i` belongs to `slot`
    /// ([`crate::context::LIFT_MIN`]…[`crate::context::LIFT_MAX`]): its own
    /// part of the day ([`Taste::part_lift_at`]) and its profile's slot
    /// ([`TimeModel::profile_lift`]), so a song never heard in the morning
    /// rises there with its morning profile. 0 without a history.
    pub fn time_bonus_at(&self, i: usize, slot: TimeSlot) -> f32 {
        let own = self.taste.part_lift_at(i, slot.part);
        let profile = self.profile_at(i).map_or(0.0, |j| self.time.profile_lift(j, slot));
        crate::context::TIME_TRACK_SHARE * own + (1.0 - crate::context::TIME_TRACK_SHARE) * profile
    }

    /// How well the sound of the track at `i` fits what the user plays in
    /// `slot` ([`TimeModel::sound_fit`]); 0 without a sound.
    pub fn sound_fit_at(&self, i: usize, slot: TimeSlot) -> f32 {
        self.sound_point_at(i).map_or(0.0, |p| self.time.sound_fit(slot, p))
    }

    /// [`Self::sound_fit_at`] of an embedded sound `v` (a new song's preview).
    pub fn sound_fit_of(&self, v: &Vector, slot: TimeSlot) -> f32 {
        self.time.sound_fit(slot, self.sound_point_of(v))
    }

    /// The share of profile `j` in `slot`, smoothed by the part of the day
    /// and everything ([`TimeModel::profile_share`]).
    pub fn profile_slot_share(&self, j: usize, slot: TimeSlot) -> f32 {
        self.time.profile_share(j, slot)
    }

    /// The share of the user's starts in `part` of the day ([`DayPart::index`]);
    /// a quarter without a history.
    pub fn part_share(&self, part: usize) -> f32 {
        let all: u32 = self.part_total.iter().sum();
        if all == 0 { 0.25 } else { self.part_total[part] as f32 / all as f32 }
    }

    pub(super) fn part_of(&self, ts: UnixMs) -> usize {
        DayPart::at(ts, crate::context::offset_at(&self.offsets, self.utc_offset_min, ts)).index()
    }
}
