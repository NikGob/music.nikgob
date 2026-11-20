use super::*;

/// The item under the pointer at height `y`, from each item's drawn
/// `(top, height, room)` (`None` when it is not drawn). Hit areas reach
/// `reach` past a slot, so neighbours (and background voices hugging
/// their line) can share a point: the slot the pointer is really in wins,
/// else the nearest one; a wait without room never takes it.
pub(super) fn hit_target(items: &[Option<(f32, f32, f32)>], y: f32, reach: f32) -> Option<usize> {
    let off = |top: f32, h: f32| if y < top { top - y } else { (y - top - h).max(0.0) };
    items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let (top, h, room) = (*it)?;
            let d = off(top, h);
            (room >= ROOMLESS && d <= reach).then_some((i, d, (y - top - h * 0.5).abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(i, ..)| i)
}

/// The position the lyrics are painted at: `lead` seconds ahead of `pos`
/// (behind when negative), within ±0.5 s and never before the start.
pub(super) fn led(pos: Duration, lead: f32) -> Duration {
    let lead = lead.clamp(-0.5, 0.5);
    if lead >= 0.0 {
        pos + Duration::from_secs_f32(lead)
    } else {
        pos.saturating_sub(Duration::from_secs_f32(-lead))
    }
}

/// The view's own clock: where the song is, as the frames see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Clock {
    /// The time of the frame it was last read in.
    pub(super) at: f64,
    /// The song's position then, seconds.
    pub(super) pos: f64,
}

/// The clock comes to the player's position over about this long, seconds.
pub(super) const CLOCK_BLEND: f64 = 0.15;
/// Further than this from the player's position (a seek, another track, a
/// loop) the clock goes there at once.
pub(super) const CLOCK_SEEK: f64 = 0.25;
/// While it waits for the player's position to catch up, the clock still
/// runs at this share of the pace: it slows down, never stops or goes back.
pub(super) const CLOCK_SLOWEST: f64 = 0.5;

impl Clock {
    /// The clock a frame on: at `now` (the frame's time) the player says
    /// `heard` seconds and moves `rate` seconds a second. While it plays
    /// the clock runs by the frames' own time, the one the springs go by,
    /// and comes to the player's position little by little: that one is
    /// read some way into each frame and is put right now and then, which
    /// made every front and jump tremble or hop. `end`: the track's length.
    pub(super) fn step(last: Option<Clock>, heard: f64, now: f64, rate: f64, end: Option<f64>, playing: bool) -> Clock {
        let there = Clock { at: now, pos: heard };
        let Some(last) = last.filter(|_| playing) else { return there };
        let dt = now - last.at;
        if dt.is_nan() || dt < 0.0 {
            return there;
        }
        let run = dt * rate.max(0.0);
        let off = heard - (last.pos + run);
        if off.is_nan() || off.abs() > CLOCK_SEEK {
            return there;
        }
        let step = (run + off * (1.0 - (-dt / CLOCK_BLEND).exp())).max(run * CLOCK_SLOWEST);
        let pos = last.pos + step;
        Clock { at: now, pos: end.map_or(pos, |end| pos.min(end.max(last.pos))) }
    }
}

/// The song's position for this frame of the view under `id` ([`Clock`]).
pub(super) fn clock(ctx: &egui::Context, id: egui::Id, player: &nikgob_view::PlayerView, now: f64, playing: bool) -> Duration {
    let heard = player.position_now().as_secs_f64();
    let end = player.duration.map(|d| d.as_secs_f64());
    let c = Clock::step(ctx.data(|d| d.get_temp(id)), heard, now, f64::from(player.rate), end, playing);
    ctx.data_mut(|d| d.insert_temp(id, c));
    Duration::from_secs_f64(c.pos.max(0.0))
}

/// The painted position, kept from stepping back by a hair while playing:
/// the clock may snap back a little as it follows what is heard, which
/// read as a front flickering backwards; a seek goes further.
pub(super) fn steady(ctx: &egui::Context, id: egui::Id, pos: Duration, playing: bool) -> Duration {
    let pos = steady_pos(ctx.data(|d| d.get_temp(id)), pos, playing);
    ctx.data_mut(|d| d.insert_temp(id, pos));
    pos
}

pub(super) fn steady_pos(last: Option<Duration>, pos: Duration, playing: bool) -> Duration {
    match last {
        Some(last) if playing && pos < last && last - pos <= STEP_BACK => last,
        _ => pos,
    }
}

/// Further back than this the position is a seek, and is followed.
pub(super) const STEP_BACK: Duration = Duration::from_millis(350);

/// Where a line that is no longer current is sung up to, from where it was
/// sung the frame before (`last`): on with the song as it plays, but a
/// seek either way (further than [`STEP_BACK`]) leaves it as it was while
/// it fades.
pub(super) fn sung_pos(last: Option<Duration>, pos: Duration) -> Duration {
    match last {
        Some(last) if pos < last || pos - last > STEP_BACK => last,
        _ => pos,
    }
}

pub(super) fn returning_offset(offset: f32, until: f64, now: f64) -> f32 {
    let t = ((now - until) / 0.5).clamp(0.0, 1.0) as f32;
    offset * (1.0 - t * t * (3.0 - 2.0 * t))
}

/// The user's own scroll of the lyrics: `offset` from following the current
/// line, held until `until` (then eased back over half a second), and the
/// follow position (`from`) it was last measured against.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Manual {
    pub(super) offset: f32,
    pub(super) until: f64,
    pub(super) from: f32,
}

impl Manual {
    /// One frame. `follow`: where following the current line scrolls to
    /// now; `dy`: the user's scroll this frame; `hold`: how long a scroll
    /// holds, seconds; `range`: the offsets that keep the text's first and
    /// last lines reachable. While it holds, the text stays where the user
    /// left it as the song moves on (a new current line moves `follow`, the
    /// offset takes the move back), so the line under the pointer is the
    /// line that gets clicked.
    pub(super) fn step(self, follow: f32, dy: f32, now: f64, hold: f64, range: (f32, f32)) -> Self {
        let mut m = self;
        if m.on(now) {
            m.offset += m.from - follow;
        }
        if dy != 0.0 {
            m.offset = m.shift(now) - dy;
            m.until = now + hold;
        }
        // Not `clamp`: it panics on an empty range.
        m.offset = m.offset.max(range.0).min(range.1);
        m.from = follow;
        m
    }

    pub(super) fn on(&self, now: f64) -> bool {
        now < self.until
    }

    /// The offset as drawn (easing back once the hold is over).
    pub(super) fn shift(&self, now: f64) -> f32 {
        returning_offset(self.offset, self.until, now)
    }
}

/// Where a click on an item seeks: a line's start; a wait's end less a
/// second, to hear the singing come.
pub(super) fn seek_to(lines: &[LyricLine], item: DisplayLine) -> Duration {
    match item {
        DisplayLine::Line(j) => lines[j].start,
        DisplayLine::Interlude { end, .. } => end.saturating_sub(Duration::from_secs(1)),
    }
}

/// A clicked item, current from the click until its seek has come back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Jump {
    pub(super) item: DisplayLine,
    /// Where it seeks ([`seek_to`]).
    pub(super) to: Duration,
    /// When it was clicked (the frame's time).
    pub(super) at: f64,
}

/// A click holds its item at most this long while the seek does not come
/// back (nothing plays).
pub(super) const JUMP_HOLD: f64 = 3.0;

/// Past the seek's point by up to this, the painted position has reached
/// the clicked item; further on it is still the position before the seek.
pub(super) const JUMP_REACH: Duration = Duration::from_millis(1500);

/// The current item with a click's `jump`: the clicked item, from the
/// click until the painted position `pos` has reached the seek's point (the
/// seek comes back a frame or more after the click; with a lead behind the
/// sound, `back`, the painted position comes up to it a little later, and
/// waits there while paused). At most [`JUMP_HOLD`] while the seek has not
/// come back; dropped when the text changed. Returns the current item and
/// the jump while it holds; without one, `by_pos`.
pub(super) fn with_jump(
    jump: Option<Jump>,
    lines: &[LyricLine],
    items: &[DisplayLine],
    by_pos: Option<usize>,
    pos: Duration,
    back: Duration,
    now: f64,
) -> (Option<usize>, Option<Jump>) {
    let Some(j) = jump else { return (by_pos, None) };
    let Some(i) = items.iter().position(|it| *it == j.item).filter(|i| seek_to(lines, items[*i]) == j.to) else {
        return (by_pos, None);
    };
    if pos >= j.to && pos <= j.to + JUMP_REACH {
        return (by_pos, None);
    }
    let lagging = pos < j.to && pos + back >= j.to;
    if !lagging && !(j.at..=j.at + JUMP_HOLD).contains(&now) {
        return (by_pos, None);
    }
    (Some(i), Some(j))
}

/// The painted position while a click's `jump` holds: where its seek will
/// land, with the `lead`, until the painted position `pos` is coming up to
/// the seek's point from behind (`back`, a lead behind the sound), which is
/// then followed; `pos` without a jump.
pub(super) fn jump_pos(jump: Option<Jump>, pos: Duration, back: Duration, lead: f32) -> Duration {
    match jump {
        Some(j) if !(pos < j.to && pos + back >= j.to) => led(j.to, lead),
        _ => pos,
    }
}

/// How long a line without an end time takes to sing: ~15 letters a second,
/// at least 0.8 s, but never past 92 % of the `gap` (seconds) to the next line.
/// Word timing for a line that only has a start: its words share a
/// speaking-pace span (see [`speaking_secs`]) in proportion to their letters.
pub(super) fn paced_words(l: &LyricLine, next: Option<Duration>) -> LyricLine {
    let gap = l
        .end
        .or(next)
        .map_or(6.0, |t| t.saturating_sub(l.start).as_secs_f32());
    let span = if l.end.is_some() { gap.max(0.3) } else { speaking_secs(&l.text, gap) };
    let letters = |w: &str| w.chars().filter(|c| !c.is_whitespace()).count() as f32;
    let total = letters(&l.text).max(1.0);
    let mut out = l.clone();
    let mut at = 0.0;
    for w in l.text.split_inclusive(char::is_whitespace) {
        let n = letters(w);
        let (a, b) = (at / total * span, (at + n) / total * span);
        at += n;
        out.words.push(nikgob_view::LyricWord {
            start: l.start + Duration::from_secs_f32(a),
            end: l.start + Duration::from_secs_f32(b),
            text: w.to_owned(),
        });
    }
    out
}

pub(super) fn speaking_secs(text: &str, gap: f32) -> f32 {
    let letters = text.chars().filter(|c| !c.is_whitespace()).count() as f32;
    // Not `clamp`: with lines closer than 0.87 s the cap is the smaller
    // bound, and `clamp` panics on that.
    (0.4 + letters * 0.066).max(0.8).min((gap * 0.92).max(0.3))
}

/// A key for each item that is the same for the same line in another
/// copy of the text: its words (and whether it is a background voice) and
/// how many times they came before; a wait by the line it waits for.
pub(super) fn item_keys(lines: &[LyricLine], items: &[DisplayLine]) -> Vec<u64> {
    use std::hash::{Hash, Hasher};
    let hash = |v: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        v(&mut h);
        h.finish()
    };
    let mut seen = std::collections::HashMap::<(&str, bool), u32>::new();
    let mut keys: Vec<u64> = items
        .iter()
        .map(|item| match *item {
            DisplayLine::Line(j) => {
                let l = &lines[j];
                let n = seen.entry((l.text.trim(), l.background)).or_default();
                *n += 1;
                hash(&|h| ("line", l.text.trim(), l.background, *n).hash(h))
            }
            DisplayLine::Interlude { .. } => 0,
        })
        .collect();
    let mut next = hash(&|h| "end".hash(h));
    for (k, item) in keys.iter_mut().zip(items).rev() {
        match item {
            DisplayLine::Line(_) => next = *k,
            DisplayLine::Interlude { .. } => *k = hash(&|h| ("wait", next).hash(h)),
        }
    }
    keys
}

/// The items' slots, kept under `id` from frame to frame.
#[derive(Clone, Default)]
pub(super) struct Slots {
    pub(super) keys: Vec<u64>,
    pub(super) slots: Vec<u64>,
    pub(super) next: u64,
}

/// The slot of each item with `keys`: when most of them were there the
/// frame before (the same song's text, swapped), each keeps its slot and a
/// new one gets a slot of its own; otherwise (another song) the slots go
/// by place, as the items do. With whether the items changed since the
/// frame before (`None`: the view's first frame).
pub(super) fn slots(ctx: &egui::Context, id: egui::Id, keys: &[u64]) -> (Vec<u64>, Option<bool>) {
    let was = ctx.data(|d| d.get_temp::<Slots>(id));
    if let Some(was) = &was
        && was.keys == keys
    {
        return (was.slots.clone(), Some(false));
    }
    let changed = was.is_some().then_some(true);
    let now = carry_slots(&was.unwrap_or_default(), keys);
    let out = now.slots.clone();
    ctx.data_mut(|d| d.insert_temp(id, now));
    (out, changed)
}

pub(super) fn carry_slots(was: &Slots, keys: &[u64]) -> Slots {
    let mut next = was.next.max(was.slots.iter().max().map_or(0, |m| m + 1));
    let mut fresh = || {
        next += 1;
        next - 1
    };
    let old: std::collections::HashMap<u64, u64> = was.keys.iter().copied().zip(was.slots.iter().copied()).collect();
    let kept = keys.iter().filter(|k| old.contains_key(k)).count();
    let slots = if !was.keys.is_empty() && kept * 2 >= keys.len() {
        keys.iter().map(|k| old.get(k).copied().unwrap_or_else(&mut fresh)).collect()
    } else {
        (0..keys.len()).map(|i| was.slots.get(i).copied().unwrap_or_else(&mut fresh)).collect()
    };
    Slots { keys: keys.to_vec(), slots, next }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DisplayLine {
    Line(usize),
    Interlude { start: Duration, end: Duration, duet: bool },
}

pub(super) fn current_item(lines: &[LyricLine], items: &[DisplayLine], pos: Duration) -> Option<usize> {
    items.iter().position(|item| matches!(item, DisplayLine::Interlude { start, end, .. } if *start <= pos && pos < *end))
        .or_else(|| items.iter().rposition(|item| matches!(item, DisplayLine::Line(i) if !lines[*i].background && lines[*i].start <= pos)))
}

/// Empty stamps own their gap, so computed waiting slots cannot double them.
/// Any singing voice overlapping a break makes it unsuitable for waiting.
pub(super) fn display_lines(lines: &[LyricLine]) -> Vec<DisplayLine> {
    let mut items = Vec::new();
    let mut prev: Option<usize> = None;
    for (i, next) in lines.iter().enumerate().filter(|(_, l)| !l.background && !l.text.trim().is_empty()) {
        let from = prev.map_or(0, |j| j + 1);
        let explicit = lines[from..i].iter().find(|l| l.text.trim().is_empty());
        let start = if let Some(empty) = explicit {
            Some(empty.start)
        } else if let Some(j) = prev {
            let l = &lines[j];
            let gap = next.start.saturating_sub(l.start).as_secs_f32();
            let end = l.end.unwrap_or(l.start + Duration::from_secs_f32(speaking_secs(&l.text, gap)));
            (next.start.saturating_sub(end) >= Duration::from_secs(4)).then_some(end)
        } else {
            (next.start >= Duration::from_millis(2500)).then_some(Duration::ZERO)
        };
        if let Some(start) = start {
            let singing = lines.iter().any(|l| {
                if !l.background || l.text.trim().is_empty() {
                    return false;
                }
                let end = l.end.unwrap_or(l.start + Duration::from_secs_f32(speaking_secs(
                    &l.text, next.start.saturating_sub(l.start).as_secs_f32(),
                )));
                l.start < next.start && end > start
            });
            if !singing && next.start.saturating_sub(start) >= Duration::from_millis(1500) {
                items.push(DisplayLine::Interlude { start, end: next.start, duet: next.duet });
            }
        }
        prev = Some(i);
    }
    items.extend(lines.iter().enumerate().filter(|(_, l)| !l.text.trim().is_empty()).map(|(i, _)| DisplayLine::Line(i)));
    items.sort_by_key(|item| match *item {
        DisplayLine::Line(i) => lines[i].start,
        DisplayLine::Interlude { start, .. } => start,
    });
    items
}

#[derive(Clone)]
pub(super) struct Breaks {
    pub(super) text: String,
    pub(super) wrap: f32,
    pub(super) big: f32,
    pub(super) forced: String,
    pub(super) height: f32,
    /// Width of the widest row at BIG.
    pub(super) width: f32,
}

/// Glyph advances round differently at different sizes. Freeze the BIG
/// rows as paragraphs: newlines are row separators, never extra glyphs.
///
/// Kept shared, so taking it every frame for every line copies no text.
pub(super) fn fixed_breaks(painter: &egui::Painter, id: egui::Id, text: &str, wrap: f32, big: f32) -> Arc<Breaks> {
    let ctx = painter.ctx();
    if let Some(b) = ctx.data(|d| d.get_temp::<Arc<Breaks>>(id))
        && b.text == text && b.wrap == wrap && b.big == big
    {
        return b;
    }
    let mut job = LayoutJob::single_section(text.to_owned(), style_at(big).text_format(Color32::WHITE));
    job.wrap.max_width = wrap;
    let g = crate::typography::layout(painter, job);
    let forced = g.rows.iter().map(|r| r.row.glyphs.iter().map(|gl| gl.chr).collect::<String>()).collect::<Vec<_>>().join("\n");
    let size = fixed_layout(painter, &forced, big).size();
    let b = Arc::new(Breaks { text: text.to_owned(), wrap, big, forced, height: size.y, width: size.x });
    ctx.data_mut(|d| d.insert_temp(id, b.clone()));
    b
}

pub(super) fn fixed_layout(painter: &egui::Painter, text: &str, size: f32) -> Arc<Galley> {
    crate::typography::layout(painter, LayoutJob::single_section(text.to_owned(), style_at(size).text_format(Color32::WHITE)))
}

/// [`fixed_layout`] for a line whose size moves between its resting sizes
/// `rest` (small, big), `t` (0..1) of the way. It is the big resting
/// layout scaled down to the size (the small one scaled up within half a
/// point of it): one layout all the way, so each letter keeps its place in
/// the line as it grows. Laid out anew at every whole size on the way, each
/// with its letters on its own whole pixels, the letters shook against
/// each other a dozen times a move, and each size drew its glyphs into the
/// font atlas the first time. Its rows and height go evenly between the two
/// resting layouts', so the line grows without the whole-pixel steps of
/// each size's own rounded row height (which moved it in its slot by half
/// a pixel at a time) and meets either resting layout where it ends.
pub(super) fn moving_layout(painter: &egui::Painter, text: &str, rest: (f32, f32), t: f32) -> Arc<Galley> {
    let size = rest.0 + (rest.1 - rest.0) * t;
    let (from, to) = (fixed_layout(painter, text, rest.0), fixed_layout(painter, text, rest.1));
    let (base, at) = if (size - rest.0).abs() <= 0.5 && (size - rest.1).abs() > 0.5 { (&from, rest.0) } else { (&to, rest.1) };
    let g = crate::typography::scale_galley(base, size / at.max(f32::EPSILON));
    between(g, &from, &to, t)
}

/// `g` with its rows placed and its height taken `t` (0..1) of the way
/// from `from`'s to `to`'s (the same rows at two sizes); as it is when the
/// rows differ.
pub(super) fn between(g: Arc<Galley>, from: &Galley, to: &Galley, t: f32) -> Arc<Galley> {
    if g.rows.len() != from.rows.len() || g.rows.len() != to.rows.len() {
        return g;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let mut out = Arc::unwrap_or_clone(g);
    let mut bounds = Rect::NOTHING;
    for (r, placed) in out.rows.iter_mut().enumerate() {
        placed.pos.y = lerp(from.rows[r].pos.y, to.rows[r].pos.y);
        bounds = bounds.union(placed.row.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
    }
    out.mesh_bounds = bounds;
    out.rect = Rect::from_min_size(out.rect.min, vec2(out.rect.width(), lerp(from.rect.height(), to.rect.height())));
    Arc::new(out)
}
