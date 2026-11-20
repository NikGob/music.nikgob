use super::*;

/// The next `n` of a wave of `filter` at `model`'s moment after `session`,
/// over the dataset's open pool.
pub fn wave(parts: &Parts, model: &UserModel, suits: &[bool], pool: &[OpenCandidate], filter: &WaveFilter, session: &Session, n: usize) -> crate::open_world::WaveNext {
    let artist_of: HashMap<String, ArtistId> = parts.new.iter().filter_map(|d| Some((crate::stream_song_key(&d.track), d.artist?))).collect();
    let artist_tracks = |t: &StreamTrack| artist_of.get(&crate::stream_song_key(t)).map(|a| model.of_artist(*a).iter().map(|&i| i as usize).collect()).unwrap_or_default();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let genre_of = genres(parts);
    let genre = |i: usize| genre_of.get(i).copied().flatten();
    let ctx = WaveContext { model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &artist_tracks, index: Some(&parts.index), learned: None };
    crate::open_world::next(&ctx, pool, filter, session, n, &HashSet::new())
}

/// The novelties of a wave session ([`crate::wave`]).
pub const WAVE_NOVELTIES: [f32; 4] = [crate::wave::NOVELTY_FAMILIAR, crate::wave::NOVELTY_BALANCE, crate::wave::NOVELTY_MORE, crate::wave::NOVELTY_ONLY_NEW];

/// What one session of «Моя волна» did ([`wave_session`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaveRun {
    /// The share of new music of each novelty of [`WAVE_NOVELTIES`]
    /// (`None`: nothing played).
    pub new: [Option<f32>; 4],
    /// The clusters it weighed.
    pub clusters: usize,
    /// Early skips of a fresh wave's songs, one after another (its vibe not
    /// settled): whether it left its cluster within [`WAVE_SKIPS_MAX`]
    /// (`None` with one cluster), and after how many.
    pub moved: Option<bool>,
    pub skips: usize,
    /// The verdicts of those skips: rejected, did not land, «not now».
    pub verdicts: [usize; 3],
}

/// The early skips of a fresh wave [`wave_session`] tries at most.
pub const WAVE_SKIPS_MAX: usize = 10;
/// The songs a wave keeps queued ahead in [`wave_session`].
const WAVE_QUEUE: usize = 3;

/// A session of «Моя волна» after `session` over the dataset's open
/// `pool`: [`WAVE_LEN`] songs at each novelty, three at a time, each heard
/// out; then at «Баланс» a fresh wave whose songs are skipped early one
/// after another as the runtime has it: each judged as the session judges
/// it (a known song rejected, an unfamiliar one did not land, a loved one
/// «not now»), and the queue's tail picked again (all but the next at one
/// skip running, all at two), until the wave leaves its cluster.
pub fn wave_session(parts: &Parts, model: &UserModel, suits: &[bool], pool: &[OpenCandidate], session: &Session) -> WaveRun {
    use crate::radio::Outcome;
    use crate::wave::WaveSeed;
    let artist_of: HashMap<String, ArtistId> = parts.new.iter().filter_map(|d| Some((crate::stream_song_key(&d.track), d.artist?))).collect();
    let artist_tracks = |t: &StreamTrack| artist_of.get(&crate::stream_song_key(t)).map(|a| model.of_artist(*a).iter().map(|&i| i as usize).collect()).unwrap_or_default();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let genre_of = genres(parts);
    let genre = |i: usize| genre_of.get(i).copied().flatten();
    let ctx = WaveContext { model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &artist_tracks, index: Some(&parts.index), learned: None };
    let mut run = WaveRun::default();
    for (k, &novelty) in WAVE_NOVELTIES.iter().enumerate() {
        let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(novelty)));
        radio.attune(session.clone(), Some(model.weights));
        let (mut played, mut new) = (0usize, 0usize);
        while played < WAVE_LEN {
            let picks = radio.extend_wave(&ctx, pool, 3, 11);
            if picks.is_empty() {
                break;
            }
            for p in picks {
                played += 1;
                new += usize::from(matches!(p.item, Item::Stream(_)));
                radio.feedback(p.item, Outcome::Complete);
            }
        }
        run.new[k] = (played > 0).then(|| new as f32 / played as f32);
        if k == 1 {
            run.clusters = radio.wave_bandit().map_or(0, |b| b.arms.len());
        }
    }
    // Early skips of a fresh wave as the runtime has them.
    let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(crate::wave::NOVELTY_BALANCE)));
    let mut heard = session.clone();
    radio.attune(heard.clone(), Some(model.weights));
    let mut queue: Vec<Item> = radio.extend_wave(&ctx, pool, WAVE_QUEUE, 13).into_iter().map(|p| p.item).collect();
    let scene = |radio: &RadioState, i: &Item| radio.wave_bandit().and_then(|b| b.scene_of(i));
    let start = queue.first().and_then(|i| scene(&radio, i));
    if start.is_some() && queue.len() >= 2 && radio.wave_bandit().is_some_and(|b| b.arms.len() >= 2) {
        let mut ts = heard.last_ts().unwrap_or_else(|| model.now());
        let mut moved = false;
        for n in 1..=WAVE_SKIPS_MAX {
            if queue.is_empty() {
                break;
            }
            let item = queue.remove(0);
            ts += 20_000;
            match skip_heard(&mut radio, &mut heard, model, item, ts) {
                Verdict::Missed => run.verdicts[1] += 1,
                Verdict::NotNow => run.verdicts[2] += 1,
                _ => run.verdicts[0] += 1,
            }
            run.skips = n;
            // The session handed over before the picks, as the runtime does.
            radio.attune(heard.clone(), Some(model.weights));
            requeue(&mut radio, &ctx, pool, &mut queue, WAVE_QUEUE, 13);
            // Moved: the song to play next is of another cluster.
            if queue.first().and_then(|i| scene(&radio, i)).is_some_and(|s| Some(s) != start) {
                moved = true;
                break;
            }
        }
        run.moved = Some(moved);
    }
    run
}

/// An early skip at 8 s of the wave's `item` at `ts` as the runtime has it:
/// the session `heard` hears it (a library song with its length, a new one
/// by an id of its own) and judges it, and the radio learns that verdict,
/// which it returns.
fn skip_heard(radio: &mut RadioState, heard: &mut Session, model: &UserModel, item: Item, ts: UnixMs) -> Verdict {
    let (id, duration_ms) = match &item {
        Item::Track(t) => (*t, crate::session::TrackLookup::duration(model, *t)),
        Item::Stream(_) => (TrackId::new(), None),
    };
    heard.start(id, ts, EventContext::Radio, Some(EventOrigin::Radio));
    heard.end_at(id, Ended::Early, 8_000, duration_ms, None);
    let verdict = heard.last().map_or(Verdict::Rejected, |p| Session::verdict(p, model));
    radio.judged(item, crate::radio::Outcome::Skip { position_ms: 8_000, duration_ms, scrub: None }, verdict);
    verdict
}

/// The wave's `queue` after a skip as the runtime leaves it: its next pick
/// kept only as [`crate::wave::Bandit::keep_next`] says, the rest
/// forgotten, then filled up to `want` again.
pub(super) fn requeue(radio: &mut RadioState, ctx: &WaveContext<'_>, pool: &[OpenCandidate], queue: &mut Vec<Item>, want: usize, seed: u64) {
    let keep = usize::from(radio.wave_bandit().is_none_or(|b| b.keep_next(queue.first()))).min(queue.len());
    let gone = queue.split_off(keep);
    radio.forget(&gone);
    let more = want.saturating_sub(queue.len());
    queue.extend(radio.extend_wave(ctx, pool, more, seed).into_iter().map(|p| p.item));
}

/// One pick of a wave replayed against the history ([`wave_replay`]).
#[derive(Clone, Debug, PartialEq)]
pub struct WaveStep {
    pub item: Item,
    /// Its catalog position (a library pick).
    pub track: Option<usize>,
    /// Its main artist: a library artist's id, or a new one by name.
    pub artist: String,
    /// The history before the moment says it is skipped early (at least
    /// half its plays): the replay ended it so. A new song's end is not
    /// known: never skipped, and the replay tells the wave nothing of it.
    pub skipped: bool,
}

/// The picks of a wave replay that went by, how many were new, how many
/// skipped (see [`wave_replay`]).
pub fn wave_counts(steps: &[WaveStep]) -> (usize, usize, usize) {
    (steps.len(), steps.iter().filter(|s| s.track.is_none()).count(), steps.iter().filter(|s| s.skipped).count())
}

/// «Моя волна» of `novelty` after `session` over the dataset's open
/// `pool`, replayed against the history: `n` picks taken three at a time
/// as the queue asks for them, each library song ended as what was known
/// of it at the moment says the user ends it — `skipped` (most of its
/// plays before the moment skipped early; the caller must not tell what
/// came after) is skipped early, any other heard out — so the wave's
/// bandit and the model's session move as they would. A new song's end is
/// unknown: the wave is told nothing of it (it is counted apart, never as
/// heard out).
///
/// What it does not measure: how new songs are received (the pool is the
/// dataset's fixed list; its picks are not matched to the history, so no
/// hit or skip of a new song counts, and «replay-skipped» is a share of all
/// picks, new ones included, so a larger share of new songs lowers it by
/// itself); and anything of the runtime — the services' requests, the
/// top-ups, previews, the log's bans — none of which a replay makes.
/// «replay-skipped» says what the history predicts of the library's picks
/// (most of their plays before the moment skipped early), not an outcome.
#[allow(clippy::too_many_arguments)]
pub fn wave_replay(parts: &Parts, model: &UserModel, suits: &[bool], pool: &[OpenCandidate], session: &Session, novelty: f32, n: usize, skipped: &dyn Fn(usize) -> bool) -> Vec<WaveStep> {
    wave_replay_seeded(parts, model, suits, pool, session, novelty, n, skipped, 11)
}

/// [`wave_replay`] of a wave whose run is seeded by `seed` (the runtime
/// seeds each wave started anew).
#[allow(clippy::too_many_arguments)]
pub fn wave_replay_seeded(
    parts: &Parts,
    model: &UserModel,
    suits: &[bool],
    pool: &[OpenCandidate],
    session: &Session,
    novelty: f32,
    n: usize,
    skipped: &dyn Fn(usize) -> bool,
    seed: u64,
) -> Vec<WaveStep> {
    use crate::radio::Outcome;
    use crate::wave::WaveSeed;
    let artist_of: HashMap<String, ArtistId> = parts.new.iter().filter_map(|d| Some((crate::stream_song_key(&d.track), d.artist?))).collect();
    let artist_tracks = |t: &StreamTrack| artist_of.get(&crate::stream_song_key(t)).map(|a| model.of_artist(*a).iter().map(|&i| i as usize).collect()).unwrap_or_default();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let genre_of = genres(parts);
    let genre = |i: usize| genre_of.get(i).copied().flatten();
    let ctx = WaveContext { model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &artist_tracks, index: Some(&parts.index), learned: None };
    let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(novelty)));
    radio.attune(session.clone(), Some(model.weights));
    let mut out: Vec<WaveStep> = Vec::with_capacity(n);
    while out.len() < n {
        let picks = radio.extend_wave(&ctx, pool, 3.min(n - out.len()), seed);
        if picks.is_empty() {
            break;
        }
        for p in picks {
            let track = match &p.item {
                Item::Track(id) => parts.catalog.position(*id),
                Item::Stream(_) => None,
            };
            // One identity of an artist for library and new songs, as the
            // wave keeps them apart ([`ArtistKey`]); an unknown one apart.
            let artist = match (&p.item, track) {
                (_, Some(i)) => ArtistKey::of_track(model, i).label(|| format!("track:{i}")),
                (Item::Stream(r), None) => pool.iter().find(|c| c.track.id == *r).map_or_else(|| r.to_string(), |c| ArtistKey::of_new(&ctx, c).label(|| format!("new:{}", c.key))),
                (Item::Track(id), None) => id.to_string(),
            };
            let skip = track.is_some_and(skipped);
            if track.is_some() {
                radio.feedback(p.item.clone(), if skip { Outcome::skip(8_000) } else { Outcome::Complete });
            } else {
                // A new song played, its end unknown: the queue moves on.
                radio.note_played(p.item.clone());
            }
            out.push(WaveStep { item: p.item, track, artist, skipped: skip });
        }
    }
    out
}

/// A session of «Моя волна» of `novelty` replayed against the history
/// ([`wave_replay`], `n` picks) the morning after the history ends, as
/// lines: each pick (K known / N new, its artist, «skip» when the replay
/// skipped it) and what they add up to — known and new, distinct artists,
/// the most picks of one artist and the longest run of one, the skipped.
pub fn wave_trace(parts: &Parts, novelty: f32, n: usize) -> String {
    let events = &parts.input.events;
    let Some(last) = events.iter().map(|e| e.ts).max() else { return String::new() };
    let offset = i64::from(parts.input.utc_offset_min) * 60_000;
    let t = ((last + offset).div_euclid(DAY_MS) + 1) * DAY_MS + 9 * HOUR_MS - offset;
    let model = parts.model_at(t);
    let suits = parts.suits();
    let pool = open_pool(parts, &model);
    // The whole history is before the morning after it.
    let plays = skips_before(&Starts::of(events), parts, t);
    let skipped = |i: usize| plays[i].0 >= 2 && 2 * plays[i].1 >= plays[i].0;
    let steps = wave_replay(parts, &model, &suits, &pool, &Session::default(), novelty, n, &skipped);
    let scale = model.scale().max(1e-6);
    // Every pick's sound, a new song's by its preview: bliss in the model's
    // space for both (the one space a library song and a preview share).
    let sounds = step_sounds(&steps, &pool, &|i| model.embedded_at(i).copied(), &|v| model.embed(v));
    let dist = |a: usize, b: usize| match (&sounds[a], &sounds[b]) {
        (Some(x), Some(y)) => Some(euclid(x, y) / scale),
        _ => None,
    };
    let mut out = format!("wave trace at novelty {novelty:.1}, pool {}:\n", pool.len());
    let mut per: HashMap<&str, usize> = HashMap::new();
    let (mut run, mut best_run) = (0usize, 0usize);
    // The sound's steps between picks one after another (both with a
    // sound; the others are counted as not measured).
    let (mut steps_sum, mut steps_n, mut jumps) = (0.0f32, 0usize, 0usize);
    for (k, s) in steps.iter().enumerate() {
        *per.entry(s.artist.as_str()).or_default() += 1;
        run = if k > 0 && steps[k - 1].artist == s.artist { run + 1 } else { 1 };
        best_run = best_run.max(run);
        let what = match &s.item {
            Item::Stream(r) => pool.iter().find(|c| c.track.id == *r).map_or_else(String::new, |c| c.track.title.clone()),
            Item::Track(_) => String::new(),
        };
        let step = if k > 0 { dist(k - 1, k) } else { None };
        if let Some(d) = step {
            steps_sum += d;
            steps_n += 1;
            jumps += usize::from(d > TRACE_JUMP);
        }
        let profile = s.track.and_then(|i| model.profile_at(i)).map_or_else(|| "-".to_owned(), |p| format!("p{p}"));
        let step = step.map_or_else(|| "    ".to_owned(), |d| format!("{d:.2}"));
        let short: String = s.artist.chars().rev().take(12).collect::<Vec<char>>().into_iter().rev().collect();
        out += &format!(
            "  {:>2} {} {:<12} {:>3} {} {}{}\n",
            k + 1,
            if s.track.is_some() { "K" } else { "N" },
            short,
            profile,
            step,
            if s.skipped { "skip " } else { "" },
            what
        );
    }
    // A skipped sound back: a pick within the next six near a skipped one.
    let mut back = 0usize;
    for (k, s) in steps.iter().enumerate() {
        if !s.skipped {
            continue;
        }
        back += (k + 1..steps.len()).take(6).filter(|&j| dist(k, j).is_some_and(|d| d < TRACE_NEAR)).count();
    }
    // The start again: the first six picks of waves started five times.
    let starts: Vec<HashSet<Item>> = (1..=5u64)
        .map(|seed| wave_replay_seeded(parts, &model, &suits, &pool, &Session::default(), novelty, 6, &skipped, seed).into_iter().map(|s| s.item).collect())
        .collect();
    let (mut same, mut pairs) = (0.0f32, 0usize);
    for a in 0..starts.len() {
        for b in a + 1..starts.len() {
            let union = starts[a].union(&starts[b]).count().max(1);
            same += starts[a].intersection(&starts[b]).count() as f32 / union as f32;
            pairs += 1;
        }
    }
    let (all, new, skip) = wave_counts(&steps);
    out += &format!(
        "  = {all} picks: known {} new {new}, artists {}, most of one artist {}, longest run {best_run}, skipped {skip}\n",
        all - new,
        per.len(),
        per.values().copied().max().unwrap_or(0)
    );
    out += &format!(
        "  = sound (bliss, library and new songs): mean step {:.2} over {steps_n} of {} steps measured, jumps over {TRACE_JUMP} {jumps}, skipped sound back within six {back}; five starts share {:.2} of their first six\n",
        steps_sum / steps_n.max(1) as f32,
        steps.len().saturating_sub(1),
        same / pairs.max(1) as f32
    );
    out
}

/// A step of the sound between two picks over this many median distances
/// is a jump ([`wave_trace`])…
const TRACE_JUMP: f32 = 1.5;
/// …and a pick this near a skipped one brings its sound back.
const TRACE_NEAR: f32 = 0.5;

/// [`kept_taste`]: each pick more of an artist already picked counts this
/// much of the one before…
pub(super) const Q_ARTIST: f32 = 0.6;
/// …a pick near a sound skipped within the six before costs this…
pub(super) const Q_BACK: f32 = 0.15;
const Q_BACK_LOOK: usize = 6;
/// …a loved song rested this long and brought back counts this much (as
/// much as it is not skipped)…
pub(super) const Q_RESTED_MS: i64 = 7 * DAY_MS;
pub(super) const Q_REDISCOVERED: f32 = 0.75;
/// …and a new song of the services, whose end the replay does not know,
/// this much (the same for every ranking: it is the novelty's share).
pub(super) const Q_NEW: f32 = 0.5;

/// The sound (bliss, in the model's space) of each of `steps`: a library
/// pick's by `lib`, a new song's by its preview in `pool` (`embed` makes it
/// the model's); none when it has none.
pub(super) fn step_sounds(steps: &[WaveStep], pool: &[OpenCandidate], lib: &dyn Fn(usize) -> Option<Vector>, embed: &dyn Fn(&Vector) -> Vector) -> Vec<Option<Vector>> {
    steps
        .iter()
        .map(|s| match (&s.item, s.track) {
            (_, Some(i)) => lib(i),
            (Item::Stream(r), None) => pool.iter().find(|c| c.track.id == *r).and_then(|c| c.vector.as_ref()).map(embed),
            (Item::Track(_), None) => None,
        })
        .collect()
}

/// The taste a wave's replayed picks kept: the mean over its picks of the
/// relevance of each not skipped (`relevance` of a library pick, [`Q_NEW`]
/// of a new song), each more of one artist counting [`Q_ARTIST`] of the
/// one before, less [`Q_BACK`] for a pick whose sound (`sounds`, in median
/// distances `scale`) is within [`TRACE_NEAR`] of a skipped pick among the
/// [`Q_BACK_LOOK`] before. A wave of a user's staples scores what it is
/// listened to, minus its repeats; a loved song brought back counts though
/// the week after did not play it.
pub(super) fn kept_taste(steps: &[WaveStep], sounds: &[Option<Vector>], scale: f32, relevance: &dyn Fn(usize) -> f32) -> f32 {
    if steps.is_empty() {
        return 0.0;
    }
    let scale = scale.max(1e-6);
    let mut per: HashMap<&str, i32> = HashMap::new();
    let mut sum = 0.0f32;
    for (t, s) in steps.iter().enumerate() {
        let c = per.entry(s.artist.as_str()).or_default();
        if !s.skipped {
            sum += Q_ARTIST.powi(*c) * s.track.map_or(Q_NEW, relevance);
        }
        *c += 1;
        let from = t.saturating_sub(Q_BACK_LOOK);
        let back = (from..t).any(|k| {
            steps[k].skipped
                && match (&sounds[k], &sounds[t]) {
                    (Some(a), Some(b)) => euclid(a, b) / scale < TRACE_NEAR,
                    _ => false,
                }
        });
        if back {
            sum -= Q_BACK;
        }
    }
    sum / steps.len() as f32
}

/// Each catalog track's plays started from `t` on and ended, and the early
/// skips among them (what the user did with it after the moment).
pub(super) fn skips_after(starts: &Starts, parts: &Parts, t: UnixMs) -> Vec<(u32, u32)> {
    let mut out = vec![(0u32, 0u32); parts.catalog.len()];
    let from = starts.list.partition_point(|s| s.0 < t);
    for s in &starts.list[from..] {
        if s.2 == Ended::Open {
            continue;
        }
        if let Some(i) = parts.catalog.position(s.1) {
            out[i].0 += 1;
            out[i].1 += u32::from(s.2 == Ended::Early);
        }
    }
    out
}

/// Each catalog track's plays ended by `t` and the early skips among them
/// (a play started before `t` and ended after it is not known yet).
pub(super) fn skips_before(starts: &Starts, parts: &Parts, t: UnixMs) -> Vec<(u32, u32)> {
    let mut out = vec![(0u32, 0u32); parts.catalog.len()];
    let upto = starts.list.partition_point(|s| s.0 < t);
    for (s, &end) in starts.list[..upto].iter().zip(&starts.ends) {
        if let Some(i) = parts.catalog.position(s.1)
            && s.2 != Ended::Open
            && end <= t
        {
            out[i].0 += 1;
            out[i].1 += u32::from(s.2 == Ended::Early);
        }
    }
    out
}

/// Accumulates [`WaveSessionStats`].
#[derive(Default)]
pub(super) struct WaveSessions {
    n: usize,
    new: [(f32, usize); 4],
    clusters: usize,
    /// Sessions measured, those moved, the skips those took.
    skips: (usize, usize, usize),
    verdicts: [usize; 3],
}

impl WaveSessions {
    pub(super) fn add(&mut self, r: &WaveRun) {
        self.n += 1;
        for (acc, x) in self.new.iter_mut().zip(r.new) {
            if let Some(x) = x {
                acc.0 += x;
                acc.1 += 1;
            }
        }
        self.clusters += r.clusters;
        if let Some(m) = r.moved {
            self.skips.0 += 1;
            self.skips.1 += usize::from(m);
            self.skips.2 += if m { r.skips } else { 0 };
        }
        for (acc, x) in self.verdicts.iter_mut().zip(r.verdicts) {
            *acc += x;
        }
    }

    pub(super) fn stats(&self) -> WaveSessionStats {
        WaveSessionStats {
            sessions: self.n,
            new: self.new.map(|(s, n)| s / n.max(1) as f32),
            clusters: self.clusters as f32 / self.n.max(1) as f32,
            skips_n: self.skips.0,
            skips_moved: self.skips.1 as f32 / self.skips.0.max(1) as f32,
            skips_to_move: self.skips.2 as f32 / self.skips.1.max(1) as f32,
            verdicts: self.verdicts,
        }
    }
}

/// Accumulates [`WaveStats`].
#[derive(Default)]
pub(super) struct Waves {
    n: usize,
    new: f32,
    niche: usize,
    artists: f32,
    per_pick: f32,
    fitting: f32,
}

impl Waves {
    pub(super) fn add(&mut self, w: &crate::open_world::WaveNext) {
        if w.picks.is_empty() {
            return;
        }
        let k = w.picks.len() as f32;
        self.n += 1;
        self.new += w.picks.iter().filter(|p| p.new).count() as f32 / k;
        self.niche += usize::from(w.niche);
        let artists: HashSet<&str> = w.picks.iter().map(|p| p.artist.as_str()).collect();
        self.artists += artists.len() as f32;
        self.per_pick += artists.len() as f32 / k;
        self.fitting += w.fitting as f32;
    }

    pub(super) fn stats(&self) -> WaveStats {
        let n = self.n.max(1) as f32;
        WaveStats { lists: self.n, new: self.new / n, niche: self.niche as f32 / n, artists: self.artists / n, artists_per_pick: self.per_pick / n, fitting: self.fitting / n }
    }
}

/// Picks of a wave the evaluation asks.
pub const WAVE_LEN: usize = 20;

/// The main artists of the tracks `model` saw started: whether the track at
/// `i` is by one of them.
pub(super) fn artist_heard(catalog: &Catalog, model: &UserModel) -> impl Fn(usize) -> bool {
    let played: HashSet<ArtistId> = (0..catalog.len().min(model.tracks.len()))
        .filter(|&i| model.tracks[i].starts > 0)
        .filter_map(|i| catalog.tracks()[i].main_artist())
        .collect();
    let mains: Vec<Option<ArtistId>> = catalog.tracks().iter().map(|t| t.main_artist()).collect();
    move |i: usize| mains.get(i).copied().flatten().is_some_and(|a| played.contains(&a))
}

/// The autoplay after the session `session` (its last track the seed) of
/// `model`: the next `n` as the runtime's autoplay picks them.
pub fn autoplay(parts: &Parts, model: &UserModel, suits: &[bool], session: Session, n: usize) -> Vec<usize> {
    let Some(seed) = session.last().map(|p| p.track) else { return Vec::new() };
    let recs = Recs::new(&parts.catalog, &model.taste, &parts.index).with_model(model).with_background(suits);
    let mut radio = RadioState::new(RadioSeed::Track(seed)).continuing();
    radio.attune(session, Some(model.weights));
    radio.set_no_repeat_ms(3 * HOUR_MS);
    radio.set_explore(Some(0.1));
    radio.set_unheard(0.0);
    let picks = radio.extend(&recs, n, &RadioParams { new_share: 0.0, seed: 1 });
    positions(&parts.catalog, picks.into_iter().map(|p| p.item))
}

/// Picks after two skips are measured by this many.
const AFTER_SKIPS: usize = 3;

/// Two early skips of autoplay's first two `picks` after `session`:
/// autoplay picks [`AFTER_SKIPS`] again (its queue replaced, as the runtime
/// does). Whether the first of them is of another taste profile than both
/// skipped (`None` when one of the three has none), and whether they sound
/// farther from the skipped two (their middle) than the picks it had queued
/// after them. `None` without enough picks or sound.
pub fn two_skips(parts: &Parts, model: &UserModel, suits: &[bool], session: &Session, picks: &[usize]) -> Option<(Option<bool>, bool)> {
    if picks.len() < 2 + AFTER_SKIPS {
        return None;
    }
    let seed = session.last()?.track;
    let ids: Vec<TrackId> = picks.iter().map(|&i| parts.catalog.tracks()[i].id).collect();
    let mut s = session.clone();
    let mut ts = s.last_ts()?;
    for &t in &ids[..2] {
        ts += 20_000;
        s.start(t, ts, EventContext::Radio, Some(EventOrigin::Autoplay));
        s.end(t, Ended::Early);
    }
    let recs = Recs::new(&parts.catalog, &model.taste, &parts.index).with_model(model).with_background(suits);
    let mut radio = RadioState::new(RadioSeed::Track(seed)).continuing();
    radio.attune(s, Some(model.weights));
    radio.set_no_repeat_ms(3 * HOUR_MS);
    radio.set_explore(Some(0.1));
    radio.set_unheard(0.0);
    radio.mark_served(ids[..2].iter().map(|t| Item::Track(*t)));
    let again = positions(&parts.catalog, radio.extend(&recs, AFTER_SKIPS, &RadioParams { new_share: 0.0, seed: 1 }).into_iter().map(|p| p.item));
    let first = *again.first()?;
    let other = match (model.profile_at(first), model.profile_at(picks[0]), model.profile_at(picks[1])) {
        (Some(a), Some(b), Some(c)) => Some(a != b && a != c),
        _ => None,
    };
    let (a, b) = (model.embedded_at(picks[0])?, model.embedded_at(picks[1])?);
    let mid: Vector = std::array::from_fn(|k| (a[k] + b[k]) / 2.0);
    let far = |list: &[usize]| -> Option<f32> {
        let d: Vec<f32> = list.iter().filter_map(|&i| model.embedded_at(i)).map(|v| euclid(v, &mid)).collect();
        (!d.is_empty()).then(|| d.iter().sum::<f32>() / d.len() as f32)
    };
    let away = far(&again)? > far(&picks[2..2 + AFTER_SKIPS])?;
    Some((other, away))
}
