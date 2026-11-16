//! Who played, where, and when: the context of a play.
//!
//! * **Who and where** ([`event_weights`]): every event of the history
//!   weighs by who started its play ([`PlayActor`]: the owner, the voice
//!   assistant, an agent of the owner's; a guest's plays never come here)
//!   and the kind of device it played on ([`PlayDevice`]: a personal one —
//!   a computer, a phone — or one in a room — a Raspberry Pi with a
//!   speaker, a TV), against the device the recommendations are made for
//!   (the target). A play has a role: chosen (a start the user picked: a
//!   song named to the assistant, the first of an album asked for), an
//!   explicit skip («дальше», a pick of another song), or passive (the
//!   queue, the radio, an album going on, and how each of them ended). The
//!   owner's own plays on a personal device weigh whole on a personal
//!   target; everything else by [`base_weight`].
//! * **Marathons**: a passive play of another kind of device or of someone
//!   else than the owner weighs less the longer the run of passive plays
//!   since the last live action of its device's session (a chosen start,
//!   an explicit skip, a seek, a like): `1 / (1 + k / τ)` for the k-th,
//!   τ [`TAU_RUN_AMBIENT`] (on a personal target) or
//!   [`TAU_RUN_TARGET_AMBIENT`] (on a room target). Eight hours of a
//!   speaker untouched weigh as about five songs heard at the computer.
//!   The owner's own runs at a computer are left to the background
//!   detection of [`crate::listening`] («Фон меньше влияет»).
//! * **When** ([`TimeSlot`], [`TimeModel`]): the part of the day and the
//!   day of the week (a day begins at 06:00, a night belongs to the day it
//!   started), the masses of the plays in each of the 28 slots halving in
//!   [`SLOT_HALF_LIFE_MS`], per taste profile and in all, smoothed slot →
//!   part of the day → everything (empirical Bayes: a Saturday morning
//!   unlike a Tuesday's separates by itself after a few weeks, a slot with
//!   little of its own leans on its part of the day), and the sound of
//!   each slot (energy, tempo, steadiness of bliss) learnt from the user's
//!   own plays: no rule of what a morning should sound like.

use std::collections::HashMap;

use nikgob_core::model::{EndReason, EventOrigin, EventType, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::taste::{DAY_MS, DayPart, Play, SESSION_GAP_MS};

/// Who started a play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlayActor {
    /// The owner (the window, the owner's token or device).
    #[default]
    Owner,
    /// The voice assistant.
    Assistant,
    /// An agent playing for the owner (an MCP client).
    Agent,
    /// A guest: never taught by (the library keeps these out).
    Guest,
}

impl PlayActor {
    /// The actor of an event's `actor` (none: the owner).
    pub fn of(actor: Option<nikgob_core::settings::ApiActor>) -> Self {
        use nikgob_core::settings::ApiActor;
        match actor {
            None | Some(ApiActor::Owner) => Self::Owner,
            Some(ApiActor::Assistant) => Self::Assistant,
            Some(ApiActor::Agent) => Self::Agent,
            Some(ApiActor::Guest) => Self::Guest,
        }
    }
}

/// The kind of device a play sounded on, or the recommendations are made
/// for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlayDevice {
    /// A computer or a phone: headphones, someone at the screen. A device
    /// the history does not know is taken for one.
    #[default]
    Personal,
    /// A speaker in a room (a Raspberry Pi), a TV, a server.
    Ambient,
}

impl PlayDevice {
    /// The kind of device of a mesh kind's word (`pc`, `pi`, `phone`, `tv`,
    /// `server`); anything else is personal.
    pub fn of_kind(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "pi" | "tv" | "server" => Self::Ambient,
            _ => Self::Personal,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// What a play was ([`event_weights`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A start the user picked ([`EventOrigin::Manual`]).
    Chosen,
    /// «Дальше» or a pick of another song ([`EndReason::UserNext`],
    /// [`EndReason::UserPick`]).
    Skipped,
    /// Whatever went on by itself, and how it ended.
    Passive,
}

/// A play weighing at least this counts in the windows of the tiles («На
/// повторе», «Год назад», worn songs) and in the counts of the part of the
/// day; one weighing less teaches its weights only.
pub const FULL_PLAY: f32 = 0.5;
/// The run of passive plays of another device or actor halves at this
/// many plays on a personal target…
pub const TAU_RUN_AMBIENT: f32 = 8.0;
/// …and every passive run at this many on a room target.
pub const TAU_RUN_TARGET_AMBIENT: f32 = 12.0;
/// The time habits halve in this.
pub const SLOT_HALF_LIFE_MS: i64 = 60 * DAY_MS;
/// A slot (a part of a weekday) leans on its part of the day by this many
/// plays…
pub const KAPPA_DOW: f32 = 6.0;
/// …a part of the day on everything by this many…
pub const KAPPA_PART: f32 = 8.0;
/// …and a track's own part of the day on everyone's by this many.
pub const KAPPA_TRACK_PART: f32 = 4.0;
/// The sound of a slot leans on its part of the day's by this many plays,
/// the part's on everything by this many…
pub const KAPPA_SOUND_DOW: f32 = 8.0;
pub const KAPPA_SOUND_PART: f32 = 12.0;
/// …and the spread of its energy is at least this.
pub const SOUND_MIN_VAR: f32 = 0.45;
/// A track's time bonus: this much its own part of the day, the rest its
/// profile's slot.
pub const TIME_TRACK_SHARE: f32 = 0.65;
/// The lift of a part or a slot over the even one, at least and at most.
pub const LIFT_MIN: f32 = -0.5;
pub const LIFT_MAX: f32 = 1.2;
/// The sound's fit to a slot: this times `1 − D²`, at least and at most…
pub const SOUND_FIT_SCALE: f32 = 0.35;
pub const SOUND_FIT_MIN: f32 = -0.45;
pub const SOUND_FIT_MAX: f32 = 0.35;
/// …trusted by the part's plays (half at this many)…
pub const SOUND_CONF_MASS: f32 = 10.0;
/// …and by how far the slot's sound stands from the user's whole (all of
/// it from this distance, never less than [`SOUND_CONF_FLOOR`]).
pub const SOUND_CONF_SPREAD: f32 = 0.40;
pub const SOUND_CONF_FLOOR: f32 = 0.15;
/// A night with fewer plays than this leans a little calmer than the
/// user's whole (energy, tempo, steadiness).
pub const NIGHT_PRIOR_UNTIL: f32 = 5.0;
pub const NIGHT_PRIOR: [f32; 3] = [-0.30, -0.15, 0.20];
/// The weights of the sound's axes in a slot's distance: energy, tempo,
/// steadiness.
const SOUND_AXES: [f32; 3] = [0.55, 0.25, 0.20];
/// «Моя волна»: the time bonus and the sound's fit beside a song's pull.
pub const WAVE_TIME_WEIGHT: f32 = 0.18;
pub const WAVE_SOUND_WEIGHT: f32 = 0.15;
/// In a room, a song this unsteady in its loudness (steadiness, z) or
/// more…
pub const AMBIENT_STEADINESS_FLOOR: f32 = -1.2;
/// …stands this much lower per unit beyond (at most 1.5 units).
pub const AMBIENT_DYN_PENALTY: f32 = 0.20;
/// In a room a mix holds this share of songs never played at most.
pub const AMBIENT_MAX_UNHEARD: f32 = 0.20;

/// The weight of a play of `actor` on `device` in `role`, the
/// recommendations made for `target` (see the module docs):
///
/// | who, where | chosen | skipped | passive, personal target | passive, room target |
/// |---|---|---|---|---|
/// | owner, personal | 1 | 1 | 1 | 0.75 |
/// | owner, room | 0.85 | 1 | 0.35 | 0.65 |
/// | assistant | 0.75 | 0.9 | 0.2 | 0.6 |
/// | agent | 0.5 | 0.5 | 0.15 | 0.25 |
/// | guest | 0 | 0 | 0 | 0 |
pub fn base_weight(actor: PlayActor, device: PlayDevice, role: Role, target: PlayDevice) -> f32 {
    let room = target == PlayDevice::Ambient;
    match (actor, device, role) {
        (PlayActor::Guest, _, _) => 0.0,
        (PlayActor::Owner, PlayDevice::Personal, Role::Passive) => {
            if room {
                0.75
            } else {
                1.0
            }
        }
        (PlayActor::Owner, PlayDevice::Personal, _) => 1.0,
        (PlayActor::Owner, PlayDevice::Ambient, Role::Chosen) => 0.85,
        (PlayActor::Owner, PlayDevice::Ambient, Role::Skipped) => 1.0,
        (PlayActor::Owner, PlayDevice::Ambient, Role::Passive) => {
            if room {
                0.65
            } else {
                0.35
            }
        }
        (PlayActor::Assistant, _, Role::Chosen) => 0.75,
        (PlayActor::Assistant, _, Role::Skipped) => 0.9,
        (PlayActor::Assistant, _, Role::Passive) => {
            if room {
                0.6
            } else {
                0.2
            }
        }
        (PlayActor::Agent, _, Role::Chosen | Role::Skipped) => 0.5,
        (PlayActor::Agent, _, Role::Passive) => {
            if room {
                0.25
            } else {
                0.15
            }
        }
    }
}

/// The τ of a passive run of `actor` on `device` for `target` (`None`: no
/// damping, the owner's own plays at a personal device for a personal
/// one).
pub fn run_tau(actor: PlayActor, device: PlayDevice, target: PlayDevice) -> Option<f32> {
    if target == PlayDevice::Ambient {
        Some(TAU_RUN_TARGET_AMBIENT)
    } else if actor == PlayActor::Owner && device == PlayDevice::Personal {
        None
    } else {
        Some(TAU_RUN_AMBIENT)
    }
}

/// What is left of a passive play's weight `k` plays into its run.
pub fn run_damping(k: u32, tau: Option<f32>) -> f32 {
    match tau {
        Some(t) if t > 0.0 => 1.0 / (1.0 + k as f32 / t),
        _ => 1.0,
    }
}

/// The role of a start (`origin` its own).
fn start_role(origin: Option<EventOrigin>) -> Role {
    if origin == Some(EventOrigin::Manual) { Role::Chosen } else { Role::Passive }
}

/// Whether an end is an explicit skip.
fn explicit_skip(p: &Play) -> bool {
    p.kind == EventType::Skip && matches!(p.end_reason, Some(EndReason::UserNext | EndReason::UserPick))
}

/// The weight of each event of `events` (in their order) for the
/// recommendations of `target` (see the module docs): a start by its
/// role, its end as its start (an explicit skip as itself), a seek and a
/// like whole. A guest's events weigh 0. The owner's own history at a
/// personal device for a personal target is all ones (and is not walked).
pub fn event_weights(events: &[Play], target: PlayDevice) -> Vec<f32> {
    let n = events.len();
    if target == PlayDevice::Personal && events.iter().all(|p| p.actor == PlayActor::Owner && p.device == PlayDevice::Personal) {
        return vec![1.0; n];
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&k| (events[k].ts, events[k].kind == EventType::Start));
    // The end of each start and the start of each end.
    let mut open: HashMap<(TrackId, PlayDevice), usize> = HashMap::new();
    let mut start_of: Vec<Option<usize>> = vec![None; n];
    for &k in &order {
        let p = &events[k];
        match p.kind {
            EventType::Start => {
                open.insert((p.track, p.device), k);
            }
            EventType::Complete | EventType::Skip => start_of[k] = open.remove(&(p.track, p.device)),
            _ => {}
        }
    }
    // The passive run of each kind of device: its length and the last
    // event of its session.
    let mut run: [(u32, Option<UnixMs>); 2] = [(0, None); 2];
    let mut w = vec![1.0f32; n];
    for &k in &order {
        let p = &events[k];
        let r = &mut run[p.device.index()];
        if r.1.is_none_or(|l| p.ts - l >= SESSION_GAP_MS) {
            r.0 = 0;
        }
        r.1 = Some(p.ts);
        if p.actor == PlayActor::Guest {
            w[k] = 0.0;
            continue;
        }
        let passive = |r: &mut (u32, Option<UnixMs>)| {
            let x = base_weight(p.actor, p.device, Role::Passive, target) * run_damping(r.0, run_tau(p.actor, p.device, target));
            r.0 += 1;
            x
        };
        w[k] = match p.kind {
            EventType::Start => match start_role(p.origin) {
                Role::Chosen => {
                    r.0 = 0;
                    base_weight(p.actor, p.device, Role::Chosen, target)
                }
                _ => passive(r),
            },
            EventType::Complete | EventType::Skip if explicit_skip(p) => {
                r.0 = 0;
                base_weight(p.actor, p.device, Role::Skipped, target)
            }
            EventType::Complete | EventType::Skip => match start_of[k] {
                Some(s) => w[s],
                // An end without its start (an import, a lost start).
                None => passive(r),
            },
            EventType::Seek | EventType::Like => {
                r.0 = 0;
                1.0
            }
            _ => 1.0,
        };
    }
    w
}

/// A part of a day of the week (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeSlot {
    /// 0 Monday … 6 Sunday, of the day the part belongs to (a day begins
    /// at 06:00).
    pub weekday: u8,
    pub part: DayPart,
}

impl TimeSlot {
    /// The slot of a moment, `utc_offset_min` east of UTC: 01:30 in the
    /// night from Friday to Saturday is Friday's night.
    pub fn at(ts: UnixMs, utc_offset_min: i32) -> Self {
        let day = DayPart::day_of(ts, utc_offset_min);
        // 1970-01-01 was a Thursday.
        Self { weekday: (day + 3).rem_euclid(7) as u8, part: DayPart::at(ts, utc_offset_min) }
    }

    /// The same weekday, another part of it.
    #[must_use]
    pub const fn with_part(self, part: DayPart) -> Self {
        Self { weekday: self.weekday, part }
    }

    const fn cell(self) -> (usize, usize) {
        ((self.weekday % 7) as usize, self.part.index())
    }
}

/// The mass, and the sums of the sound, of plays in one slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SoundSums {
    w: f32,
    /// Weighted sums of energy, tempo, steadiness…
    x: [f32; 3],
    /// …and of the energy squared.
    e2: f32,
}

impl SoundSums {
    fn add(&mut self, m: f32, p: [f32; 3]) {
        self.w += m;
        for (s, v) in self.x.iter_mut().zip(p) {
            *s += m * v;
        }
        self.e2 += m * p[0] * p[0];
    }

    fn plus(mut self, o: &Self) -> Self {
        self.w += o.w;
        for (s, v) in self.x.iter_mut().zip(o.x) {
            *s += v;
        }
        self.e2 += o.e2;
        self
    }

    /// The means and the energy's variance, shrunk to `prior` (means,
    /// variance) by `kappa` plays.
    fn shrunk(&self, prior: ([f32; 3], f32), kappa: f32) -> ([f32; 3], f32) {
        let d = self.w + kappa;
        if d <= 0.0 {
            return prior;
        }
        let mean: [f32; 3] = std::array::from_fn(|a| (self.x[a] + kappa * prior.0[a]) / d);
        let e2 = (self.e2 + kappa * (prior.1 + prior.0[0] * prior.0[0])) / d;
        (mean, (e2 - mean[0] * mean[0]).max(0.0))
    }
}

/// The user's time habits (see the module docs): the mass of each taste
/// profile and of all plays in each slot, and the sound of each slot.
#[derive(Clone, Debug, Default)]
pub struct TimeModel {
    profile: Vec<[[f32; 4]; 7]>,
    total: [[f32; 4]; 7],
    sound: [[SoundSums; 4]; 7],
}

impl TimeModel {
    /// An empty model of `profiles` taste profiles.
    pub fn new(profiles: usize) -> Self {
        Self { profile: vec![[[0.0; 4]; 7]; profiles], ..Self::default() }
    }

    /// A play of `mass` (its weight, its outcome, its age) in `slot`, of
    /// `profile`, sounding `sound` (energy, tempo, steadiness).
    pub fn add(&mut self, slot: TimeSlot, profile: Option<usize>, mass: f32, sound: Option<[f32; 3]>) {
        if mass <= 0.0 || !mass.is_finite() {
            return;
        }
        let (d, p) = slot.cell();
        self.total[d][p] += mass;
        if let Some(prof) = profile.and_then(|j| self.profile.get_mut(j)) {
            prof[d][p] += mass;
        }
        if let Some(s) = sound.filter(|s| s.iter().all(|x| x.is_finite())) {
            self.sound[d][p].add(mass, s);
        }
    }

    fn all(&self) -> f32 {
        self.total.iter().flatten().sum()
    }

    /// The mass of all plays in `part` of any day: how much the part is
    /// known.
    pub fn learned(&self, part: DayPart) -> f32 {
        self.total.iter().map(|d| d[part.index()]).sum()
    }

    /// The share of profile `j` in `slot`, smoothed slot → part of the
    /// day → everything; its share of everything when nothing is known of
    /// the slot (0: nothing of it at all).
    pub fn profile_share(&self, j: usize, slot: TimeSlot) -> f32 {
        let Some(prof) = self.profile.get(j) else { return 0.0 };
        let all = self.all();
        if all <= 0.0 {
            return 0.0;
        }
        let (d, p) = slot.cell();
        let mean = prof.iter().flatten().sum::<f32>() / all;
        let in_part: f32 = prof.iter().map(|x| x[p]).sum();
        let part_all: f32 = self.total.iter().map(|x| x[p]).sum();
        let part = (in_part + KAPPA_PART * mean) / (part_all + KAPPA_PART);
        (prof[d][p] + KAPPA_DOW * part) / (self.total[d][p] + KAPPA_DOW)
    }

    /// How much more than its usual share profile `j` is played in `slot`
    /// ([`LIFT_MIN`]…[`LIFT_MAX`]); 0 for a profile never played.
    pub fn profile_lift(&self, j: usize, slot: TimeSlot) -> f32 {
        let Some(prof) = self.profile.get(j) else { return 0.0 };
        let all = self.all();
        if all <= 0.0 {
            return 0.0;
        }
        let mean = prof.iter().flatten().sum::<f32>() / all;
        if mean < 1e-6 {
            return 0.0;
        }
        (self.profile_share(j, slot) / mean.max(0.03) - 1.0).clamp(LIFT_MIN, LIFT_MAX)
    }

    /// The sound the user plays in `slot` (energy, tempo, steadiness), the
    /// spread of its energy and how far it is trusted (0…1); `None` before
    /// any play with a sound.
    pub fn slot_sound(&self, slot: TimeSlot) -> Option<([f32; 3], f32, f32)> {
        let (d, p) = slot.cell();
        let whole = self.sound.iter().flatten().fold(SoundSums::default(), |a, s| a.plus(s));
        if whole.w <= 0.0 {
            return None;
        }
        let global = whole.shrunk(([0.0; 3], 1.0), 0.0);
        let part = self.sound.iter().map(|x| &x[p]).fold(SoundSums::default(), |a, s| a.plus(s));
        let prior = if slot.part == DayPart::Night && part.w < NIGHT_PRIOR_UNTIL {
            (std::array::from_fn(|a| global.0[a] + NIGHT_PRIOR[a]), global.1)
        } else {
            global
        };
        let of_part = part.shrunk(prior, KAPPA_SOUND_PART);
        let (mean, var) = self.sound[d][p].shrunk(of_part, KAPPA_SOUND_DOW);
        let away: f32 = mean.iter().zip(global.0).map(|(a, b)| (a - b).powi(2)).sum::<f32>().sqrt();
        let conf = part.w / (part.w + SOUND_CONF_MASS) * (away / SOUND_CONF_SPREAD).clamp(SOUND_CONF_FLOOR, 1.0);
        Some((mean, var, conf))
    }

    /// How well a sound `point` (energy, tempo, steadiness) fits `slot`:
    /// [`SOUND_FIT_MIN`]…[`SOUND_FIT_MAX`] times how far the slot's sound
    /// is trusted; 0 before anything is known.
    pub fn sound_fit(&self, slot: TimeSlot, point: [f32; 3]) -> f32 {
        let Some((mean, var, conf)) = self.slot_sound(slot) else { return 0.0 };
        let d2: f32 = (0..3).map(|a| SOUND_AXES[a] * (point[a] - mean[a]).powi(2)).sum::<f32>() / var.max(SOUND_MIN_VAR);
        conf * (SOUND_FIT_SCALE * (1.0 - d2)).clamp(SOUND_FIT_MIN, SOUND_FIT_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::HOUR_MS;
    use crate::test_util::tid;

    const T0: UnixMs = 20_000 * DAY_MS;

    fn play(track: u128, ts: UnixMs, kind: EventType, actor: PlayActor, device: PlayDevice) -> Play {
        Play { actor, device, ..Play::new(tid(track), ts, kind, Some(0)) }
    }

    /// The owner's own history at a computer is all ones for the
    /// computer, whatever it is.
    #[test]
    fn the_owners_own_history_weighs_whole() {
        let events: Vec<Play> = (0..50)
            .flat_map(|k| {
                let ts = T0 + k * 200_000;
                [play(k as u128, ts, EventType::Start, PlayActor::Owner, PlayDevice::Personal), play(k as u128, ts + 190_000, EventType::Complete, PlayActor::Owner, PlayDevice::Personal)]
            })
            .collect();
        assert!(event_weights(&events, PlayDevice::Personal).iter().all(|w| *w == 1.0));
        // For the speaker the computer's passive plays weigh less.
        let w = event_weights(&events, PlayDevice::Ambient);
        assert!((w[0] - 0.75).abs() < 1e-6, "{}", w[0]);
        assert!(w[99] < w[0], "a long run is damped there");
    }

    /// Eight hours of the speaker untouched after one command weigh as
    /// about five songs at the computer; the command itself counts as the
    /// owner's intent, «дальше» on the speaker whole and it starts the run
    /// again.
    #[test]
    fn a_marathon_on_the_speaker_weighs_a_few_songs() {
        let mut events = vec![Play { origin: Some(EventOrigin::Manual), ..play(0, T0, EventType::Start, PlayActor::Assistant, PlayDevice::Ambient) }];
        events.push(play(0, T0 + 180_000, EventType::Complete, PlayActor::Assistant, PlayDevice::Ambient));
        for k in 1..=160u128 {
            let ts = T0 + k as i64 * 180_000;
            events.push(Play { origin: Some(EventOrigin::Radio), ..play(k, ts, EventType::Start, PlayActor::Assistant, PlayDevice::Ambient) });
            events.push(play(k, ts + 179_000, EventType::Complete, PlayActor::Assistant, PlayDevice::Ambient));
        }
        let w = event_weights(&events, PlayDevice::Personal);
        assert!((w[0] - 0.75).abs() < 1e-6 && (w[1] - 0.75).abs() < 1e-6, "the command: {} {}", w[0], w[1]);
        let completes: f32 = (3..events.len()).step_by(2).map(|k| w[k]).sum();
        assert!(completes > 3.5 && completes < 5.5, "{completes}");
        // The first passive songs keep most of their weight.
        assert!((w[2] - 0.2).abs() < 1e-6, "{}", w[2]);
        // «Дальше» two hours in: whole, and the run starts again.
        let at = T0 + 40 * 180_000 + 60_000;
        let mut skipped = events.clone();
        skipped.retain(|p| !(p.track == tid(40) && p.kind == EventType::Complete));
        skipped.push(Play { end_reason: Some(EndReason::UserNext), ..play(40, at, EventType::Skip, PlayActor::Assistant, PlayDevice::Ambient) });
        let w2 = event_weights(&skipped, PlayDevice::Personal);
        assert!((w2[skipped.len() - 1] - 0.9).abs() < 1e-6);
        let next = skipped.iter().position(|p| p.track == tid(41) && p.kind == EventType::Start).unwrap();
        assert!((w2[next] - 0.2).abs() < 1e-6, "the run starts again: {}", w2[next]);
        // On the speaker itself its own passive songs weigh more.
        let room = event_weights(&events, PlayDevice::Ambient);
        assert!(room[2] > w[2] && (room[2] - 0.6).abs() < 1e-6);
        // A guest's weigh nothing.
        let guest = [play(1, T0, EventType::Start, PlayActor::Guest, PlayDevice::Personal)];
        assert_eq!(event_weights(&guest, PlayDevice::Personal), [0.0]);
    }

    /// A long pause starts another session: its run from the start.
    #[test]
    fn a_pause_ends_the_run() {
        let mut events = Vec::new();
        for k in 0..30u128 {
            events.push(play(k, T0 + k as i64 * 200_000, EventType::Start, PlayActor::Owner, PlayDevice::Ambient));
        }
        events.push(play(99, T0 + 30 * 200_000 + 2 * HOUR_MS, EventType::Start, PlayActor::Owner, PlayDevice::Ambient));
        let w = event_weights(&events, PlayDevice::Personal);
        assert!(w[29] < 0.4 * 0.35 + 0.1);
        assert!((w[30] - 0.35).abs() < 1e-6, "{}", w[30]);
    }

    #[test]
    fn slots_of_the_week() {
        // 1970-01-05 was a Monday; 08:00 UTC.
        let monday = 4 * DAY_MS;
        assert_eq!(TimeSlot::at(monday + 8 * HOUR_MS, 0), TimeSlot { weekday: 0, part: DayPart::Morning });
        // 01:30 in the night from Friday to Saturday is Friday's night.
        let friday = monday + 4 * DAY_MS;
        assert_eq!(TimeSlot::at(friday + DAY_MS + 90 * 60_000, 0), TimeSlot { weekday: 4, part: DayPart::Night });
        assert_eq!(TimeSlot::at(friday + DAY_MS + 9 * HOUR_MS, 0), TimeSlot { weekday: 5, part: DayPart::Morning });
        // Moscow: 21:00 UTC on Sunday is 00:00 Monday, still Sunday's night.
        assert_eq!(TimeSlot::at(monday - 3 * HOUR_MS, 180), TimeSlot { weekday: 6, part: DayPart::Night });
    }

    /// A profile played on Saturday mornings rises there, apart from the
    /// other mornings; the sound of a slot is the user's own, trusted by
    /// how far it stands from the whole.
    #[test]
    fn the_week_and_its_sound() {
        let mut t = TimeModel::new(2);
        let sat = TimeSlot { weekday: 5, part: DayPart::Morning };
        let tue = TimeSlot { weekday: 1, part: DayPart::Morning };
        let eve = TimeSlot { weekday: 1, part: DayPart::Evening };
        for _ in 0..30 {
            for d in 0..5u8 {
                t.add(TimeSlot { weekday: d, part: DayPart::Morning }, Some(0), 1.0, Some([-0.8, -0.5, 0.6]));
                t.add(TimeSlot { weekday: d, part: DayPart::Evening }, Some(1), 1.0, Some([0.9, 0.7, -0.2]));
            }
            t.add(sat, Some(1), 1.0, Some([1.0, 0.9, 0.0]));
        }
        assert!(t.profile_lift(1, sat) > 0.3, "{}", t.profile_lift(1, sat));
        assert!(t.profile_lift(0, sat) < 0.0);
        assert!(t.profile_lift(0, tue) > 0.3, "{}", t.profile_lift(0, tue));
        assert!(t.profile_share(0, tue) > t.profile_share(1, tue));
        let calm = [-0.8, -0.5, 0.6];
        let loud = [1.2, 1.0, -0.5];
        assert!(t.sound_fit(tue, calm) > 0.1, "{}", t.sound_fit(tue, calm));
        assert!(t.sound_fit(tue, loud) < 0.0);
        assert!(t.sound_fit(eve, loud) > t.sound_fit(eve, calm));
        // One sound all day: hardly trusted.
        let mut flat = TimeModel::new(1);
        for d in 0..7u8 {
            for p in DayPart::ALL {
                flat.add(TimeSlot { weekday: d, part: p }, Some(0), 5.0, Some([0.1, 0.0, 0.0]));
            }
        }
        assert!(flat.sound_fit(tue, loud).abs() <= SOUND_CONF_FLOOR * SOUND_FIT_MAX.max(-SOUND_FIT_MIN) + 1e-6);
        assert_eq!(TimeModel::new(1).sound_fit(tue, calm), 0.0, "nothing known");
    }
}
