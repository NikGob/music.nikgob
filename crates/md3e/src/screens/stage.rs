//! The live stage of Settings (settings v2 §2): the real Now Playing, its
//! lyrics, the top of Главная or an album's header, drawn a second time by
//! the same painters, smaller, under the settings being shown. Not a
//! picture and not a mock-up: the same functions, called again into a layer
//! of their own that takes no input and whose commands are dropped.
//!
//! The settings page only reserves the rect and asks for it ([`request`]);
//! the shell draws it ([`StageUi::draw`]) right after the page, in an area
//! laid just over the page's layer (`set_sublayer`) and scaled onto the
//! rect (`set_transform_layer`), the way Now Playing opens out of the
//! floating player.
//!
//! The painters learn whether they draw for real or for the stage from a
//! [`Stage`]: their springs are kept apart ([`Stage::id`]), the copy reads
//! no pointer and asks for repaints only while it moves, and it reports
//! where its parts are ([`Stage::part`]) so the row under the pointer can
//! have its part outlined and small parts brought up to 1:1.

// A few helpers are only read by the tests and the shell's checks.
#![cfg_attr(not(test), allow(dead_code))]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Context, Id, LayerId, Pos2, Rect, Stroke, Ui, UiBuilder, Vec2, emath::TSTransform, pos2, vec2};
use nikgob_motion::Spring;
use nikgob_view::{
    Command, CoverShape, LyricLine, LyricWord, NpSlot, PlayerView, Prefs, ReleaseCard, ReleaseDay, ReleaseId,
    ReleaseType, Status, TrackId, TrackRow, ViewState,
};

use super::{
    chrome::Chrome,
    now_playing,
    pages::{self, PageState, home::Section},
};
use crate::{ColorScheme, Contrast, SchemeVariant, Theme, components::anim};

/// The width the stage lays its screens out at (at least).
pub const CANVAS_W: f32 = 960.0;
/// The least height of the canvas.
pub const CANVAS_MIN_H: f32 = 560.0;
/// What sets the copy's ids apart from the real screen's.
const SALT: &str = "settings-stage";
/// Each half of a change of scene: the old one fades to the background,
/// then the new one comes in (two scenes are never drawn at once).
const FADE: f64 = 0.09;
/// How long the outline stays after the pointer leaves its row, so moving
/// between rows does not blink.
const HOLD: f64 = 0.4;
/// The stage stands still this long after the pointer last moved (unless
/// something plays).
const STILL_AFTER: f32 = 8.0;
/// The stage's own frame budget; over it, it repaints at most 60 times a
/// second.
const BUDGET_MS: f32 = 2.0;
const FRAME_60: Duration = Duration::from_micros(16_667);
/// The screen's corners on the stage (points on screen).
const CORNER: f32 = 28.0;

// ---- What the painters are told -----------------------------------------

/// How a painter draws: on screen for real ([`Stage::REAL`]), or as the
/// settings' copy.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    copy: bool,
    /// Takes the pointer, sends commands, claims the wheel and the
    /// snackbars' spot (only the real screen).
    pub live: bool,
    /// Moves: asks for repaints. A still copy is a frozen frame.
    pub moving: bool,
    /// Points on screen per point painted (for the shapes' feathers).
    pub scale: f32,
    /// Made-up levels (bass, bands); `None`: the real ones.
    pub levels: Option<(f32, Vec<f32>)>,
    /// The lyrics scrolled "by hand" this frame (a scene's play-acting), in
    /// place of the pointer's.
    pub hand: f32,
    /// The cover is held down (a scene's taps), in place of the pointer.
    pub pressed: bool,
    /// Repaints at most 60 times a second (over its budget).
    pub throttle: bool,
}

impl Stage {
    /// The real screen.
    pub const REAL: Self = Self {
        copy: false,
        live: true,
        moving: true,
        scale: 1.0,
        levels: None,
        hand: 0.0,
        pressed: false,
        throttle: false,
    };

    /// A copy for the stage at `scale`.
    pub fn copy(scale: f32, moving: bool, levels: (f32, Vec<f32>)) -> Self {
        Self { copy: true, live: false, moving, scale, levels: Some(levels), ..Self::REAL }
    }

    /// Whether this is the stage's copy.
    pub fn is_copy(&self) -> bool {
        self.copy
    }

    /// The id of the painter's state `name` (its full old name, e.g.
    /// `"np-mega"`): the same id for the real screen, its own for the copy,
    /// so the two never share a spring.
    pub fn id(&self, name: &str) -> Id {
        let id = Id::new(name);
        if self.copy { id.with(SALT) } else { id }
    }

    /// Notes where `part` was painted this frame (the copy only; the real
    /// screen costs nothing).
    pub fn part(&self, ctx: &Context, part: Part, rect: Rect) {
        if self.copy && rect.is_finite() && rect.is_positive() {
            ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_id()).0.push((part, rect)));
        }
    }

    /// Asks for the next frame while it moves.
    pub fn repaint(&self, ctx: &Context) {
        if !self.moving {
            return;
        }
        if self.throttle {
            ctx.request_repaint_after(FRAME_60);
        } else {
            ctx.request_repaint();
        }
    }

    /// Asks for a frame in `after` while it moves.
    pub fn repaint_after(&self, ctx: &Context, after: Duration) {
        if self.moving {
            ctx.request_repaint_after(if self.throttle { after.max(FRAME_60) } else { after });
        }
    }

    /// The bass level: the made-up one, else `real`.
    pub fn bass(&self, real: f32) -> f32 {
        self.levels.as_ref().map_or(real, |l| l.0)
    }

    /// The spectrum bands: the made-up ones, else `real`.
    pub fn bands(&self, real: &[f32]) -> Vec<f32> {
        self.levels.as_ref().map_or_else(|| real.to_vec(), |l| l.1.clone())
    }
}

/// Physical pixels per point of what is being painted: the window's, times
/// the stage's scale while the stage is drawn. Shape and stroke feathers
/// are one physical pixel wide, so on a stage at 0.5 they would turn into a
/// staircase with the window's alone.
pub fn ppp(ctx: &Context) -> f32 {
    ctx.pixels_per_point() * ctx.data(|d| d.get_temp::<f32>(scale_id())).unwrap_or(1.0)
}

fn scale_id() -> Id {
    Id::new(SALT).with("scale")
}

fn parts_id() -> Id {
    Id::new(SALT).with("parts")
}

/// Where the parts were painted this frame.
#[derive(Clone, Debug, Default)]
struct Parts(Vec<(Part, Rect)>);

/// A part of a screen a setting changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// All of it (the theme, the contrast): nothing to outline.
    Whole,
    Cover,
    /// The ring around the cover (its box).
    Ring,
    /// The ambient glow and the shapes behind everything.
    Background,
    Progress,
    Volume,
    Title,
    /// The line being sung.
    LyricsLine,
    /// The lyrics' column.
    LyricsColumn,
    /// The pause indicator between lines (and its countdown).
    LyricsPause,
    /// The soft bands at the top and the bottom of the lyrics.
    LyricsEdges,
    /// A section of Главная.
    Home(Section),
    /// The album's type and date line.
    AlbumDate,
}

impl Part {
    /// Small enough to be brought up to 1:1.
    pub fn zooms(self) -> bool {
        matches!(self, Self::Progress | Self::Volume | Self::LyricsLine | Self::LyricsPause | Self::AlbumDate)
    }

    /// The scene it is on (`None`: any).
    pub fn scene(self) -> Option<Scene> {
        Some(match self {
            Self::Whole => return None,
            Self::Cover | Self::Ring | Self::Background | Self::Progress | Self::Volume | Self::Title => Scene::Track,
            Self::LyricsLine | Self::LyricsColumn | Self::LyricsPause | Self::LyricsEdges => Scene::Lyrics,
            Self::Home(_) => Scene::Home,
            Self::AlbumDate => Scene::Album,
        })
    }
}

/// The screen on the stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scene {
    #[default]
    Track,
    /// Now Playing with its lyrics open.
    Lyrics,
    /// The top of Главная.
    Home,
    /// The header of the playing track's album.
    Album,
}

impl Scene {
    pub const ALL: [Self; 4] = [Self::Track, Self::Lyrics, Self::Home, Self::Album];

    /// Its name on the group of buttons under the stage.
    pub fn label(self) -> &'static str {
        match self {
            Self::Track => "Трек",
            Self::Lyrics => "Текст",
            Self::Home => "Главная",
            Self::Album => "Альбом",
        }
    }
}

/// What the stage acts out by itself, on a loop, for what cannot be seen
/// without doing something (settings v2 §2.3, «Сценки»).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Script {
    #[default]
    None,
    /// «Случайная»: another shape every 2 s.
    RandomShape,
    /// 2.5 s playing, 2 s paused.
    PausePlay,
    /// Two taps on the cover, then another shape.
    DoubleTap,
    /// The volume changes for a second, then holds (the pill waits its time
    /// and goes).
    Volume,
    /// Now Playing opens out of the player and closes again.
    Open,
    /// A step to the next line every 1.4 s.
    LyricsStep,
    /// The lyrics scrolled away by hand, then coming back.
    LyricsReturn,
    /// Slow scrolling through the soft bands.
    EdgesScroll,
    /// A 6 s break between lines, on a loop.
    Interlude,
    /// The last 4 s of the break (the countdown).
    Countdown,
    /// A text with line times only.
    LineTimes,
}

impl Script {
    /// It plays the built-in text (it needs a break, or no word times).
    fn wants_sample(self) -> bool {
        matches!(self, Self::Interlude | Self::Countdown | Self::LineTimes)
    }
}

/// What the settings page asks the stage to show this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct StageRequest {
    /// Where, in the page's (screen) coordinates.
    pub rect: Rect,
    pub scene: Scene,
    /// The settings shown: the page's, with the choice under the pointer.
    pub prefs: Prefs,
    /// The parts to outline (the row's part; all of a preset's).
    pub parts: Vec<Part>,
    /// How strongly they are outlined (0..1: the row under the pointer,
    /// then [`super::pages::settings_help::glow_at`] after a change).
    pub outline: f32,
    /// A single small part is brought up to 1:1.
    pub zoom: bool,
    pub script: Script,
    pub opacity: f32,
}

impl StageRequest {
    pub fn new(rect: Rect, scene: Scene, prefs: Prefs) -> Self {
        Self { rect, scene, prefs, parts: Vec::new(), outline: 0.0, zoom: true, script: Script::None, opacity: 1.0 }
    }
}

fn request_id() -> Id {
    Id::new(SALT).with("request")
}

/// Asks for the stage this frame (the settings page, before the shell
/// draws it). Without a request the stage is not drawn.
pub fn request(ctx: &Context, req: StageRequest) {
    ctx.data_mut(|d| d.insert_temp(request_id(), req));
}

/// What was asked for this frame and not drawn yet (tests of the page
/// without the shell).
#[cfg(test)]
pub fn requested(ctx: &Context) -> Option<StageRequest> {
    ctx.data(|d| d.get_temp::<StageRequest>(request_id()))
}

fn take(ctx: &Context) -> Option<StageRequest> {
    ctx.data_mut(|d| {
        let req = d.get_temp::<StageRequest>(request_id());
        d.remove::<StageRequest>(request_id());
        req
    })
}

// ---- Geometry -----------------------------------------------------------

/// The canvas a stage of `size` lays its screen out on, and the scale it is
/// shown at: the stage's proportions, at least [`CANVAS_W`] wide and
/// [`CANVAS_MIN_H`] tall (a low, wide stage gets a canvas wider than the
/// window: the screen's layout stays real), never larger than real.
pub fn canvas(size: Vec2) -> (Vec2, f32) {
    let scale = (size.x / CANVAS_W).min(size.y / CANVAS_MIN_H).clamp(1e-3, 1.0);
    (size / scale, scale)
}

/// Canvas → screen: the whole canvas over `rect`, or brought in on `focus`
/// by `k` (0..1): no closer than 1:1 (a part too big for that just fits),
/// and the window onto the canvas never past its edges.
pub fn to_global(rect: Rect, canvas: Vec2, focus: Option<Rect>, k: f32) -> TSTransform {
    let base = rect.width() / canvas.x.max(1.0);
    let middle = (canvas * 0.5).to_pos2();
    let (scale, centre) = match focus {
        Some(f) if k > 0.0 => {
            let k = k.clamp(0.0, 1.0);
            let fit = (rect.width() * 0.8 / f.width().max(1.0)).min(rect.height() * 0.8 / f.height().max(1.0));
            let close = fit.min(1.0).max(base);
            (base + (close - base) * k, middle.lerp(f.center(), k))
        }
        _ => (base, middle),
    };
    let window = rect.size() / scale;
    let room = (canvas - window).max(Vec2::ZERO);
    let min = pos2(
        (centre.x - window.x * 0.5).clamp(0.0, room.x),
        (centre.y - window.y * 0.5).clamp(0.0, room.y),
    );
    TSTransform::new(rect.min.to_vec2() - min.to_vec2() * scale, scale)
}

// ---- Scenes acted out ---------------------------------------------------

/// «2,5 с играет / 2 с пауза»: whether it plays `t` seconds in.
pub fn pause_playing(t: f32) -> bool {
    t.rem_euclid(4.5) < 2.5
}

/// Seconds played `t` seconds into [`pause_playing`] (the clock stops on
/// each pause).
pub fn pause_played(t: f32) -> f32 {
    let n = (t / 4.5).floor();
    n * 2.5 + (t - n * 4.5).min(2.5)
}

/// The double tap every 3 s: whether the cover is held down `t` seconds
/// in, and how many second taps have landed (each one changes the shape).
pub fn double_tap(t: f32) -> (bool, i64) {
    let phase = t.rem_euclid(3.0);
    let down = (1.0..1.12).contains(&phase) || (1.3..1.42).contains(&phase);
    (down, ((t - 1.42) / 3.0).floor() as i64 + 1)
}

/// «Случайная»: how many shapes have come `t` seconds in (one every 2 s).
pub fn random_step(t: f32) -> i64 {
    (t / 2.0).floor() as i64
}

/// The volume `t` seconds in: a second of change from `base` (up, then
/// down the next time), then still while the pill holds (`hold`) and goes.
pub fn volume_at(t: f32, base: f32, hold: f32) -> f32 {
    let period = 2.5 + hold.clamp(0.3, 5.0);
    let n = (t / period).floor();
    let x = (t - n * period).min(1.0);
    let k = x * x * (3.0 - 2.0 * x);
    let up = (n as i64).rem_euclid(2) == 0;
    base + 0.2 * if up { k } else { 1.0 - k }
}

/// Now Playing opening out of the player and closing again on its spring:
/// how open it is `t` seconds in (0..1, may overshoot a little).
pub fn open_at(t: f32, spring: Spring) -> f32 {
    const HALF: f32 = 1.6;
    let phase = t.rem_euclid(2.0 * HALF);
    if phase < HALF { 1.0 - spring.solve(1.0, 0.0, phase).0 } else { spring.solve(1.0, 0.0, phase - HALF).0 }
}

/// A step from line to line every `every` seconds: where it is `t` seconds
/// in, over the lines starting at `starts`.
pub fn stepped(t: f32, starts: &[f32], every: f32) -> f32 {
    if starts.is_empty() {
        return t;
    }
    let n = (t / every).floor();
    starts[(n as i64).rem_euclid(starts.len() as i64) as usize] + (t - n * every)
}

/// The lyrics' scroll by hand `t` seconds in (all of it so far, so each
/// frame's move is the difference): pushed away once a `cycle` and left to
/// come back by itself.
pub fn hand_at(t: f32, cycle: f32) -> f32 {
    const PUSH: f32 = 140.0;
    let n = (t / cycle).floor();
    let x = ((t - n * cycle - 0.5) / 0.6).clamp(0.0, 1.0);
    (n + x * x * (3.0 - 2.0 * x)) * PUSH
}

/// The built-in text: (start, line) — four lines, a break, four more.
const SAMPLE: [(f32, &str); 8] = [
    (1.0, "Где-то рядом"),
    (3.4, "Слышишь этот ритм"),
    (5.8, "Он ведёт меня"),
    (8.2, "Сквозь огни домов"),
    (16.5, "До самого утра"),
    (18.9, "И дальше вперёд"),
    (21.3, "Пока горит свет"),
    (23.7, "Мы не спим"),
];
/// How long each line is sung.
const SING: f32 = 1.8;
/// The built-in text loops after this.
pub const SAMPLE_LOOP: f32 = 27.0;
/// Its break between the lines.
const BREAK: (f32, f32) = (8.2 + SING, 16.5);

/// The built-in text, with word times (`words`) or line times only.
pub fn sample_lyrics(words: bool) -> Vec<LyricLine> {
    SAMPLE
        .iter()
        .map(|&(start, text)| {
            let mut line = LyricLine::new(Duration::from_secs_f32(start), text);
            if words {
                line.end = Some(Duration::from_secs_f32(start + SING));
                let letters = |w: &str| w.chars().filter(|c| !c.is_whitespace()).count() as f32;
                let total = letters(text).max(1.0);
                let mut at = 0.0;
                for w in text.split_inclusive(' ') {
                    let n = letters(w);
                    line.words.push(LyricWord {
                        start: Duration::from_secs_f32(start + at / total * SING),
                        end: Duration::from_secs_f32(start + (at + n) / total * SING),
                        text: w.to_owned(),
                    });
                    at += n;
                }
            }
            line
        })
        .collect()
}

/// The break of the built-in text on a loop, a little before and after it.
pub fn interlude_at(t: f32) -> f32 {
    let (from, to) = (BREAK.0 - 0.4, BREAK.1 + 0.6);
    from + t.rem_euclid(to - from)
}

/// The last 4 s of the break (its countdown) on a loop.
pub fn countdown_at(t: f32) -> f32 {
    let (from, to) = (BREAK.1 - 4.0, BREAK.1 + 0.6);
    from + t.rem_euclid(to - from)
}

/// Made-up levels on a 120 BPM beat, for when nothing plays.
pub fn fake_levels(t: f32) -> (f32, Vec<f32>) {
    let kick = pages::settings_help::beat(t);
    let bands = (0..32)
        .map(|i| {
            let x = i as f32 / 31.0;
            ((0.2 + 0.7 * kick * (1.0 - x).powi(2)) * (0.65 + 0.35 * (x * 9.0 + t * 3.0).sin())).clamp(0.0, 1.0)
        })
        .collect();
    (0.15 + 0.6 * kick, bands)
}

/// The day `days` after 1970-01-01 (UTC).
fn civil(days: i64) -> ReleaseDay {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    ReleaseDay { year: (yoe + era * 400 + i64::from(month <= 2)) as i32, month: month as u8, day: Some(day as u8) }
}

fn today() -> ReleaseDay {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil((secs / 86_400) as i64)
}

// ---- The state shown ----------------------------------------------------

/// «Название песни / Исполнитель» when nothing plays.
fn placeholder(id: TrackId) -> TrackRow {
    TrackRow {
        id,
        title: "Название песни".into(),
        artist: "Исполнитель".into(),
        album: Some("Альбом".into()),
        release_id: None,
        artists: Default::default(),
        disc_no: 1,
        track_no: 1,
        duration: Some(Duration::from_secs(195)),
        quality: None,
        source: None,
        has_local: true,
        liked: false,
        cover: None,
        clip: false,
    }
}

/// The state the stage draws `scene` from: the player, the lyrics, the
/// accounts and the dislikes of the real one, `prefs` the settings shown;
/// Главная and the album only for their own scenes, the library never.
pub(crate) fn light_state(state: &ViewState, prefs: &Prefs, scene: Scene, script: Script, track: TrackId) -> ViewState {
    let mut v = ViewState {
        player: state.player.clone(),
        lyrics: state.lyrics.clone(),
        accounts: state.accounts.clone(),
        dislikes: state.dislikes.clone(),
        prefs: prefs.clone(),
        revision: state.revision,
        ..Default::default()
    };
    match scene {
        Scene::Home => v.home = state.home.clone(),
        Scene::Album => v.release = state.release.clone(),
        Scene::Track | Scene::Lyrics => {}
    }
    if v.player.current.is_none() {
        v.player.current = Some(placeholder(track));
        v.player.duration = Some(Duration::from_secs(195));
    }
    if v.player.duration.is_none_or(|d| d.is_zero()) {
        v.player.duration = Some(Duration::from_secs(195));
    }
    if script.wants_sample() || v.lyrics.lines.is_empty() {
        v.lyrics.lines = sample_lyrics(script != Script::LineTimes);
        v.lyrics.plain = None;
        v.lyrics.loading = false;
    }
    v
}

/// Whether the stage plays the built-in text.
fn plays_sample(state: &ViewState, script: Script) -> bool {
    script.wants_sample() || state.lyrics.lines.is_empty()
}

/// Where each line of `lines` starts (main lines with words).
fn starts(lines: &[LyricLine]) -> Vec<f32> {
    lines.iter().filter(|l| !l.background && !l.text.trim().is_empty()).map(|l| l.start.as_secs_f32()).collect()
}

/// One frame of the stage's player: always playing (on its own `clock`
/// where the real one is not, or the scene plays the built-in text), the
/// scene's play-acting (`t` seconds into it) on top.
fn drive(v: &mut ViewState, real: &PlayerView, script: Script, t: f32, clock: f32, sample: bool) {
    let p = &mut v.player;
    let len = if sample { SAMPLE_LOOP } else { p.duration.map_or(195.0, |d| d.as_secs_f32()).max(10.0) };
    let real_playing = real.status == Status::Playing;
    let lines = starts(&v.lyrics.lines);
    let pos = match script {
        Script::Interlude => Some(interlude_at(t)),
        Script::Countdown => Some(countdown_at(t)),
        Script::LyricsStep => Some(stepped(t, &lines, 1.4)),
        Script::EdgesScroll => Some(stepped(t, &lines, 1.6)),
        Script::PausePlay => Some(pause_played(t).rem_euclid(len)),
        _ if real_playing && !sample => None,
        _ => Some(clock.rem_euclid(len)),
    };
    p.status = if script == Script::PausePlay && !pause_playing(t) { Status::Paused } else { Status::Playing };
    if let Some(pos) = pos {
        p.position = Duration::from_secs_f32(pos.max(0.0));
        p.at = Instant::now();
        p.rate = 1.0;
    }
    if script == Script::Volume {
        p.muted = false;
        p.volume = volume_at(t, real.volume.clamp(0.15, 0.75), v.prefs.np_volume_pop_hold);
    }
}

/// The album the stage's header shows.
struct Album {
    card: ReleaseCard,
    released: Option<ReleaseDay>,
    count: u32,
    total: Duration,
    facts: Vec<String>,
}

/// The playing track's album: its page's data when that is the album open,
/// else what the track knows. Without a known day, today stands in, so
/// every way of showing the date looks different.
fn album_of(v: &ViewState) -> Album {
    let current = v.player.current.as_ref();
    let open = v.release.as_ref().filter(|r| current.is_some_and(|t| t.release_id == Some(r.card.id)));
    let mut album = match open {
        Some(r) => Album {
            card: r.card.clone(),
            released: r.released,
            count: if r.lines.is_empty() { r.tracks.len() } else { r.lines.len() } as u32,
            total: r.total_duration,
            facts: r.label.iter().chain(&r.quality).cloned().collect(),
        },
        None => {
            let t = current;
            Album {
                card: ReleaseCard {
                    id: t.and_then(|t| t.release_id).unwrap_or_else(ReleaseId::new),
                    title: t.and_then(|t| t.album.clone()).unwrap_or_else(|| "Альбом".into()),
                    artist: t.map_or_else(|| "Исполнитель".into(), |t| t.artist.clone()),
                    artists: t.map(|t| t.artists.clone()).unwrap_or_default(),
                    year: None,
                    release_type: ReleaseType::Album,
                    cover: t.and_then(|t| t.cover.clone()),
                    local_tracks: 12,
                    total_tracks: Some(12),
                    liked: false,
                    added_at: 0,
                },
                released: None,
                count: 12,
                total: Duration::from_secs(42 * 60),
                facts: Vec::new(),
            }
        }
    };
    album.released = album.released.filter(|d| d.day.is_some()).or_else(|| Some(today()));
    album
}

// ---- The shell's side ---------------------------------------------------

/// The scene on the stage and the one coming.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    from: Scene,
    to: Scene,
    since: f64,
}

impl Fade {
    /// The scene drawn at `now` and its opacity: the old one fades to the
    /// background, then the new one comes in.
    fn at(self, now: f64) -> (Scene, f32) {
        if self.from == self.to {
            return (self.to, 1.0);
        }
        let dt = now - self.since;
        if dt < FADE {
            (self.from, (1.0 - dt / FADE) as f32)
        } else {
            (self.to, ((dt - FADE) / FADE).min(1.0) as f32)
        }
    }

    /// On to `scene` from whatever is drawn at `now`.
    fn toward(self, scene: Scene, now: f64) -> Self {
        if scene == self.to {
            return self;
        }
        Self { from: self.at(now).0, to: scene, since: now }
    }
}

/// The outline asked for last, held a moment after the pointer leaves.
#[derive(Clone, Debug)]
struct Held {
    parts: Vec<Part>,
    outline: f32,
    zoom: bool,
    at: f64,
}

/// The light state kept between frames.
struct Cache {
    revision: u64,
    prefs: Prefs,
    scene: Scene,
    script: Script,
    state: ViewState,
    sample: bool,
    album: Option<Album>,
}

/// The stage's presentation state, kept by the shell: a [`Chrome`] and a
/// [`PageState`] of its own (whatever the copy does to them stays there),
/// the light state and what is on the stage.
pub struct StageUi {
    chrome: Chrome,
    pages: PageState,
    cache: Option<Cache>,
    fade: Option<Fade>,
    held: Option<Held>,
    /// The part zoomed in on (kept while zooming back out).
    focus: Option<(Part, Rect)>,
    /// Where the parts were last frame.
    rects: Vec<(Part, Rect)>,
    script: Script,
    script_since: f64,
    /// Seconds into the script last frame (the hand's move is the
    /// difference).
    script_last: f32,
    reroll: i64,
    born: Option<f64>,
    track: TrackId,
    scheme: Option<((u32, bool, SchemeVariant, Contrast), ColorScheme)>,
    /// What the stage cost, ms (moving average).
    cost: f32,
    /// Drawn this frame.
    pub visible: bool,
    /// Where it was drawn this frame (on screen).
    pub rect: Option<Rect>,
}

impl Default for StageUi {
    fn default() -> Self {
        Self {
            chrome: Chrome::default(),
            pages: PageState::default(),
            cache: None,
            fade: None,
            held: None,
            focus: None,
            rects: Vec::new(),
            script: Script::None,
            script_since: 0.0,
            script_last: 0.0,
            reroll: 0,
            born: None,
            track: TrackId::new(),
            scheme: None,
            cost: 0.0,
            visible: false,
            rect: None,
        }
    }
}

impl StageUi {
    /// The theme of the settings shown: their scheme of the current cover,
    /// their reduced motion (or the system's).
    fn theme(&mut self, ctx: &Context, prefs: &Prefs, app: Theme) -> Theme {
        let seed = ctx.data(|d| d.get_temp::<u32>(super::scheme_seed_id())).unwrap_or(prefs.fallback_seed);
        let (dark, variant, contrast) = super::theme_key(ctx, prefs);
        let key = (seed, dark, variant, contrast);
        let scheme = match self.scheme {
            Some((k, s)) if k == key => s,
            _ => {
                let s = ColorScheme::from_seed(seed, variant, dark, contrast);
                self.scheme = Some((key, s));
                s
            }
        };
        Theme::new(scheme, dark).with_reduced_motion(prefs.reduced_motion || app.reduced_motion)
    }

    /// Draws the stage the settings page asked for this frame, if it did:
    /// in its own layer just over the `page` layer, clipped to the page
    /// (`clip`), faded with it (`opacity`). `levels` are the real ones.
    pub fn draw(&mut self, ctx: &Context, page: LayerId, clip: Rect, opacity: f32, state: &ViewState, levels: (f32, &[f32])) {
        self.visible = false;
        self.rect = None;
        let Some(req) = take(ctx).filter(|r| r.rect.is_positive() && r.opacity > 0.0) else {
            self.fade = None;
            return;
        };
        let _p = crate::prof::scope("settings.stage");
        let started = Instant::now();
        let now = anim::now(ctx);
        let app = Theme::get(ctx);
        let born = *self.born.get_or_insert(now);

        // The scene, through the background when it changes.
        let fade = self.fade.map_or(Fade { from: req.scene, to: req.scene, since: now }, |f| f.toward(req.scene, now));
        self.fade = Some(fade);
        let (scene, alpha) = fade.at(now);
        if alpha < 1.0 {
            ctx.request_repaint();
        }

        // What is outlined, held a moment after the pointer leaves its row.
        if !req.parts.is_empty() {
            self.held = Some(Held { parts: req.parts.clone(), outline: req.outline, zoom: req.zoom, at: now });
        }
        let held = self.held.clone().filter(|h| !req.parts.is_empty() || now - h.at < HOLD);
        if req.parts.is_empty()
            && let Some(h) = &held
        {
            ctx.request_repaint_after(Duration::from_secs_f64((h.at + HOLD - now).max(0.0) + 0.01));
        }
        let (parts, outline, zoom) = held.map_or((Vec::new(), 0.0, false), |h| (h.parts, h.outline, h.zoom));

        // A single small part: brought up to 1:1 (where it was last frame).
        let zoom_part = parts.first().copied().filter(|p| zoom && parts.len() == 1 && p.zooms());
        let found = zoom_part.and_then(|p| self.rects.iter().find(|(q, _)| *q == p).copied());
        if found.is_some() {
            self.focus = found;
        }
        let k = anim::fraction(
            ctx,
            Id::new(SALT).with("zoom"),
            if found.is_some() { 1.0 } else { 0.0 },
            app.motion.default_spatial.without_bounce(),
        );
        if k <= 0.001 {
            self.focus = None;
        }
        let (size, _) = canvas(req.rect.size());
        let virt = Rect::from_min_size(Pos2::ZERO, size);
        let global = to_global(req.rect, size, self.focus.map(|(_, r)| r), k);
        let visible = req.rect.intersect(clip);
        let local_clip = global.inverse() * visible;

        // The light state, built again only when something it shows changed.
        if req.script != self.script {
            self.script = req.script;
            self.script_since = now;
            self.script_last = 0.0;
        }
        let rebuild = self.cache.as_ref().is_none_or(|c| {
            c.revision != state.revision || c.prefs != req.prefs || c.scene != scene || c.script != req.script
        });
        if rebuild {
            let v = light_state(state, &req.prefs, scene, req.script, self.track);
            let album = (scene == Scene::Album).then(|| album_of(&v));
            self.cache = Some(Cache {
                revision: state.revision,
                prefs: req.prefs.clone(),
                scene,
                script: req.script,
                sample: plays_sample(state, req.script),
                state: v,
                album,
            });
        }
        let t = (now - self.script_since) as f32;
        let clock = (now - born) as f32;
        let Some(cache) = self.cache.as_mut() else { return };
        drive(&mut cache.state, &state.player, req.script, t, clock, cache.sample);

        // The scene's play-acting.
        let (pressed, taps) = if req.script == Script::DoubleTap { double_tap(t) } else { (false, 0) };
        // The second tap changes the shape only where the gesture is on (as
        // on the real screen: 1 — «Случайная» only, 2 — any shape).
        let choice = req.prefs.cover_shape;
        let gesture = match req.prefs.dbl_click_shape {
            0 => false,
            1 => choice == CoverShape::Random,
            _ => choice != CoverShape::Original,
        };
        let step = match req.script {
            Script::RandomShape => random_step(t),
            Script::DoubleTap if gesture => taps,
            _ => 0,
        };
        if step != self.reroll {
            self.reroll = step;
            if choice == CoverShape::Random {
                let id = Stage::copy(1.0, false, (0.0, Vec::new())).id("np-random-shape").with("reroll");
                ctx.data_mut(|d| d.insert_temp(id, true));
            }
        }
        if req.script == Script::DoubleTap && gesture && choice != CoverShape::Random {
            let pool = now_playing::random_pool(&req.prefs.random_shapes);
            let from = pool.iter().position(|c| *c == choice).unwrap_or(0);
            cache.state.prefs.cover_shape = pool[(from as i64 + taps).rem_euclid(pool.len() as i64) as usize];
        }
        let cycle = 2.6 + req.prefs.lyrics_return_s.max(0.5);
        let hand = if req.script == Script::LyricsReturn { hand_at(t, cycle) - hand_at(self.script_last, cycle) } else { 0.0 };
        self.script_last = t;

        let real_playing = state.player.status == Status::Playing;
        let levels = if real_playing && req.script != Script::PausePlay {
            (levels.0, levels.1.to_vec())
        } else {
            fake_levels(clock)
        };
        let moving = real_playing || ctx.input(|i| i.pointer.time_since_last_movement()) < STILL_AFTER;
        let mut stage = Stage::copy(global.scaling, moving, levels);
        stage.hand = hand;
        stage.pressed = pressed;
        stage.throttle = self.cost > BUDGET_MS;
        let open = (req.script == Script::Open).then(|| open_at(t, super::np_spring(req.prefs.np_motion)));

        let theme = self.theme(ctx, &req.prefs, app);
        self.chrome.np_slot = if scene == Scene::Lyrics { NpSlot::Lyrics } else { NpSlot::None };
        self.pages.levels_bass = stage.bass(0.0);
        ctx.data_mut(|d| {
            d.insert_temp(scale_id(), global.scaling);
            d.insert_temp(parts_id(), Parts::default());
        });
        let Self { chrome, pages, cache, .. } = self;
        let Some(cache) = cache.as_ref() else { return };
        // The copy's commands go nowhere.
        let mut dropped: Vec<Command> = Vec::new();
        let mut rects = Vec::new();
        let area = egui::Area::new(Id::new(SALT))
            // The page's order (`set_sublayer` wants the same one).
            .order(egui::Order::Background)
            .fixed_pos(Pos2::ZERO)
            .constrain(false)
            // A picture: neither hover nor clicks.
            .interactable(false)
            .fade_in(false)
            .show(ctx, |ui| {
                ui.set_clip_rect(local_clip);
                ui.multiply_opacity(req.opacity * opacity * alpha);
                Theme::set(ctx, theme);
                paint_scene(ui, virt, scene, &cache.state, cache.album.as_ref(), &stage, open, chrome, pages, &mut dropped);
                Theme::set(ctx, app);
                ctx.data_mut(|d| d.remove_temp::<f32>(scale_id()));
                rects = ctx.data_mut(|d| d.remove_temp::<Parts>(parts_id())).unwrap_or_default().0;
                frame(ui, global, req.rect, &rects, &parts, outline, app);
            });
        drop(dropped);
        let layer = area.response.layer_id;
        ctx.set_sublayer(page, layer);
        ctx.set_transform_layer(layer, global);
        self.rects = rects;
        self.visible = true;
        self.rect = Some(req.rect);
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        self.cost = self.cost * 0.9 + ms * 0.1;
    }
}

/// Paints `scene` from `v` over the canvas `virt`.
#[allow(clippy::too_many_arguments)]
fn paint_scene(
    ui: &mut Ui,
    virt: Rect,
    scene: Scene,
    v: &ViewState,
    album: Option<&Album>,
    stage: &Stage,
    open: Option<f32>,
    chrome: &mut Chrome,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    ui.painter().rect_filled(virt, 0.0, s.surface);
    match scene {
        Scene::Track | Scene::Lyrics => {
            let mut nui = ui.new_child(UiBuilder::new().max_rect(virt).id_salt("stage-np"));
            let Some(k) = open else {
                now_playing::show_on(&mut nui, virt, v, false, pages, chrome, stage, out);
                return;
            };
            // Out of the floating player and back, as the shell opens it:
            // the container grows from the pill, the content laid out for
            // the whole window scales up from it and fades in.
            let kc = k.clamp(0.0, 1.0);
            let pill = Rect::from_center_size(pos2(virt.center().x, virt.bottom() - 48.0), vec2((virt.width() - 32.0).min(960.0), 64.0));
            let rect = Rect::from_min_max(pill.min + (virt.min - pill.min) * kc, pill.max + (virt.max - pill.max) * kc);
            now_playing::container(ui, rect, kc);
            let pivot = pill.center().lerp(virt.center(), kc).to_vec2();
            let t = TSTransform::from_translation(pivot) * TSTransform::from_scaling(0.86 + 0.14 * k) * TSTransform::from_translation(-pivot);
            // Clips move with the shapes: revealed inside the container.
            let inv = t.inverse();
            nui.set_clip_rect((inv * rect).intersect(inv * nui.clip_rect()));
            let a = ((k - 0.12) / 0.5).clamp(0.0, 1.0);
            nui.multiply_opacity(a * a * (3.0 - 2.0 * a));
            transformed(&mut nui, t, |ui| {
                now_playing::show_on(ui, virt, v, false, pages, chrome, stage, out);
            });
            stage.part(&ctx, Part::Whole, rect);
        }
        Scene::Home => {
            // The page's margins and the search bar's room above it.
            let page = Rect::from_min_max(virt.min + vec2(24.0, 88.0), virt.max - vec2(24.0, 0.0));
            let mut hui = ui.new_child(UiBuilder::new().max_rect(page).id_salt("stage-home"));
            hui.spacing_mut().item_spacing.y = 8.0;
            pages::home::show_on(&mut hui, v, pages, stage, out);
        }
        Scene::Album => {
            let Some(a) = album else { return };
            let page = Rect::from_min_max(virt.min + vec2(24.0, 88.0), virt.max - vec2(24.0, 0.0));
            let mut aui = ui.new_child(UiBuilder::new().max_rect(page).id_salt("stage-album"));
            aui.spacing_mut().item_spacing.y = 8.0;
            let header = pages::detail::ReleaseHeader {
                card: &a.card,
                released: a.released,
                album_date: v.prefs.album_date,
                count: a.count,
                total: a.total,
                facts: a.facts.iter().map(String::as_str).collect(),
            };
            let drawn = pages::detail::release_top(&mut aui, &a.card.title, &header);
            stage.part(&ctx, Part::AlbumDate, drawn.date);
        }
    }
}

/// Runs `add`, then moves what it painted in this layer by `t`.
fn transformed(ui: &mut Ui, t: TSTransform, add: impl FnOnce(&mut Ui)) {
    let layer = ui.layer_id();
    let start = ui.ctx().graphics(|g| g.get(layer).map_or(egui::layers::ShapeIdx(0), |l| l.next_idx()));
    add(ui);
    ui.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        let end = list.next_idx();
        list.transform_range(start, end, t);
    });
}

/// Over the scene, in canvas points: the outline of the parts asked for,
/// then the screen's rounded corners (the page's surface over the corners
/// of the window onto the canvas) and its thin edge.
fn frame(ui: &Ui, global: TSTransform, rect: Rect, drawn: &[(Part, Rect)], parts: &[Part], outline: f32, app: Theme) {
    let s = app.scheme;
    let painter = ui.painter();
    let px = 1.0 / global.scaling.max(1e-3);
    if outline > 0.001 {
        for (_, r) in drawn.iter().filter(|(p, _)| *p != Part::Whole && parts.contains(p)) {
            let r = r.expand(6.0 * px);
            let radius = (12.0 * px).min(255.0);
            painter.rect_filled(r, radius, s.primary.gamma_multiply(0.18 * outline));
            painter.rect_stroke(r, radius, Stroke::new(2.0 * px, s.primary.gamma_multiply(outline)), egui::StrokeKind::Outside);
        }
    }
    let window = global.inverse() * rect;
    let radius = (CORNER * px).min(170.0);
    // A ring as wide as the radius, its inner edge the rounded window.
    painter.rect_stroke(window.expand(radius * 0.5), radius * 1.5, Stroke::new(radius, s.surface), egui::StrokeKind::Middle);
    painter.rect_stroke(window, radius, Stroke::new(px, s.outline_variant), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canvas_keeps_the_stage_proportions_and_its_least_size() {
        for size in [vec2(700.0, 440.0), vec2(760.0, 250.0), vec2(560.0, 560.0), vec2(1800.0, 1100.0)] {
            let (c, scale) = canvas(size);
            assert!(c.x >= CANVAS_W - 0.01 && c.y >= CANVAS_MIN_H - 0.01, "{size:?} -> {c:?}");
            assert!((c.x / c.y - size.x / size.y).abs() < 1e-3, "{size:?} -> {c:?}");
            assert!(scale <= 1.0);
            assert!(((c * scale) - size).length() < 0.01);
        }
        // A low, wide band: the canvas is wider than the window, not cut.
        let (c, _) = canvas(vec2(760.0, 250.0));
        assert!((c.y - CANVAS_MIN_H).abs() < 0.01 && c.x > 1500.0, "{c:?}");
    }

    #[test]
    fn canvas_and_stage_map_onto_each_other_and_back() {
        let rect = Rect::from_min_size(pos2(500.0, 140.0), vec2(700.0, 440.0));
        let (c, scale) = canvas(rect.size());
        let t = to_global(rect, c, None, 0.0);
        assert!((t.scaling - scale).abs() < 1e-5);
        assert!((t * Pos2::ZERO).distance(rect.min) < 0.01);
        assert!((t * c.to_pos2()).distance(rect.max) < 0.01);
        let back = t.inverse() * rect;
        assert!(back.min.distance(Pos2::ZERO) < 0.01 && back.max.distance(c.to_pos2()) < 0.01);
        let p = pos2(321.0, 77.0);
        assert!((t.inverse() * (t * p)).distance(p) < 0.01);
    }

    #[test]
    fn zooming_stops_at_one_to_one_and_inside_the_canvas() {
        let rect = Rect::from_min_size(pos2(500.0, 140.0), vec2(700.0, 440.0));
        let (c, base) = canvas(rect.size());
        let parts = [
            Rect::from_min_size(pos2(10.0, 10.0), vec2(120.0, 30.0)),
            Rect::from_min_size(pos2(c.x - 60.0, c.y - 20.0), vec2(50.0, 16.0)),
            Rect::from_min_size(pos2(400.0, 300.0), vec2(600.0, 40.0)),
            Rect::from_min_size(pos2(100.0, 100.0), vec2(900.0, 500.0)),
        ];
        for part in parts {
            let mut last = base;
            for i in 0..=10 {
                let t = to_global(rect, c, Some(part), i as f32 / 10.0);
                assert!(t.scaling <= 1.0 + 1e-5 && t.scaling >= base - 1e-5, "{part:?}: {}", t.scaling);
                assert!(t.scaling + 1e-5 >= last, "zooms in steadily");
                last = t.scaling;
                let window = t.inverse() * rect;
                assert!(window.min.x >= -0.01 && window.min.y >= -0.01, "{part:?} {window:?}");
                assert!(window.max.x <= c.x + 0.01 && window.max.y <= c.y + 0.01, "{part:?} {window:?}");
            }
            let t = to_global(rect, c, Some(part), 1.0);
            let window = t.inverse() * rect;
            if part.width() <= rect.width() * 0.8 && part.height() <= rect.height() * 0.8 {
                assert!(window.contains_rect(part), "the part is in view: {part:?} {window:?}");
            }
        }
    }

    #[test]
    fn scenes_act_out_on_a_loop() {
        // Pause: 2.5 s on, 2 s off; its clock stops while paused.
        assert!(pause_playing(0.0) && pause_playing(2.4) && !pause_playing(2.6) && !pause_playing(4.4) && pause_playing(4.6));
        assert!((pause_played(3.0) - 2.5).abs() < 1e-5);
        assert!((pause_played(4.4) - 2.5).abs() < 1e-5);
        assert!((pause_played(5.0) - 3.0).abs() < 1e-5);
        // Taps: two presses, then the next shape.
        assert_eq!(double_tap(0.5), (false, 0));
        assert_eq!(double_tap(1.05), (true, 0));
        assert_eq!(double_tap(1.2), (false, 0));
        assert_eq!(double_tap(1.35), (true, 0));
        assert_eq!(double_tap(1.5), (false, 1));
        assert_eq!(double_tap(4.5), (false, 2));
        assert_eq!(random_step(1.9), 0);
        assert_eq!(random_step(2.1), 1);
        // Volume: a second of change, then still while the pill holds.
        let hold = 1.0;
        assert!((volume_at(0.0, 0.5, hold) - 0.5).abs() < 1e-5);
        assert!((volume_at(1.0, 0.5, hold) - 0.7).abs() < 1e-5);
        assert_eq!(volume_at(1.5, 0.5, hold), volume_at(3.0, 0.5, hold));
        assert!((volume_at(3.5 + 1.0, 0.5, hold) - 0.5).abs() < 1e-5, "back down the next time");
        // The break: inside the text's gap (a little either side), on a loop.
        for i in 0..200 {
            let t = i as f32 * 0.1;
            let p = interlude_at(t);
            assert!(p >= BREAK.0 - 0.41 && p <= BREAK.1 + 0.61, "{t}: {p}");
            let c = countdown_at(t);
            assert!(c >= BREAK.1 - 4.01 && c <= BREAK.1 + 0.61, "{t}: {c}");
        }
        assert!(BREAK.1 - BREAK.0 >= 6.0, "a 6 s break");
        // Steps: a line every 1.4 s, round again after the last.
        let starts = [1.0, 3.0, 5.0];
        assert!((stepped(0.1, &starts, 1.4) - 1.1).abs() < 1e-5);
        assert!((stepped(1.5, &starts, 1.4) - 3.1).abs() < 1e-4);
        assert!((stepped(4.3, &starts, 1.4) - 1.1).abs() < 1e-4);
        // The hand: pushed once a cycle, never pulled back by the script.
        let mut last = hand_at(0.0, 4.0);
        for i in 1..200 {
            let h = hand_at(i as f32 * 0.05, 4.0);
            assert!(h >= last);
            last = h;
        }
        assert!(hand_at(0.4, 4.0) == 0.0 && hand_at(1.2, 4.0) > 100.0);
        // Opening: from shut to open and back on the spring.
        let spring = super::super::np_spring(1);
        assert!(open_at(0.0, spring) < 0.01 && open_at(1.5, spring) > 0.95 && open_at(3.15, spring) < 0.05);
    }

    #[test]
    fn the_built_in_text_has_a_break_and_words_on_request() {
        let words = sample_lyrics(true);
        assert_eq!(words.len(), 8);
        assert!(words.iter().all(|l| !l.words.is_empty() && l.end.is_some()));
        assert!(words.iter().all(|l| l.words.iter().map(|w| w.text.as_str()).collect::<String>() == l.text));
        let lines = sample_lyrics(false);
        assert!(lines.iter().all(|l| l.words.is_empty() && l.end.is_none()));
        assert!(SAMPLE.last().is_some_and(|(s, _)| s + SING < SAMPLE_LOOP));
    }

    #[test]
    fn the_light_state_clones_home_and_the_album_for_their_scenes_only() {
        let mut state = ViewState::default();
        state.home.loading = true;
        state.home.greeting_hour = 7;
        state.library.loaded = true;
        state.revision = 42;
        let prefs = Prefs { reduced_motion: true, ..Default::default() };
        let track = TrackId::new();
        for scene in Scene::ALL {
            let v = light_state(&state, &prefs, scene, Script::None, track);
            assert_eq!(v.home.greeting_hour == 7, scene == Scene::Home, "{scene:?}");
            assert!(!v.library.loaded, "{scene:?}: the library is never cloned");
            assert!(v.prefs.reduced_motion, "the settings shown");
            assert_eq!(v.revision, 42);
            // Nothing plays: the placeholder, with a text to sing.
            assert_eq!(v.player.current.as_ref().map(|t| t.title.as_str()), Some("Название песни"));
            assert!(v.player.duration.is_some());
            assert_eq!(v.lyrics.lines.len(), 8);
        }
        // The placeholder keeps its id, so a random shape is not drawn anew
        // on every rebuild.
        let a = light_state(&state, &prefs, Scene::Track, Script::None, track);
        let b = light_state(&state, &prefs, Scene::Track, Script::None, track);
        assert_eq!(a.player.current.map(|t| t.id), b.player.current.map(|t| t.id));
    }

    #[test]
    fn the_stage_always_plays() {
        let state = ViewState::default();
        let track = TrackId::new();
        let mut v = light_state(&state, &Prefs::default(), Scene::Track, Script::None, track);
        drive(&mut v, &state.player, Script::None, 0.0, 30.0, true);
        assert_eq!(v.player.status, Status::Playing);
        assert!((v.player.position.as_secs_f32() - (30.0 - SAMPLE_LOOP)).abs() < 0.01, "round the loop");
        drive(&mut v, &state.player, Script::PausePlay, 3.0, 30.0, true);
        assert_eq!(v.player.status, Status::Paused);
        drive(&mut v, &state.player, Script::Volume, 1.0, 30.0, true);
        assert!((v.player.volume - (state.player.volume.clamp(0.15, 0.75) + 0.2)).abs() < 1e-4);
    }

    #[test]
    fn a_change_of_scene_fades_through_the_background() {
        let f = Fade { from: Scene::Track, to: Scene::Track, since: 0.0 };
        assert_eq!(f.at(5.0), (Scene::Track, 1.0));
        let f = f.toward(Scene::Lyrics, 10.0);
        assert_eq!(f.at(10.0), (Scene::Track, 1.0));
        let (s, a) = f.at(10.045);
        assert!(s == Scene::Track && (a - 0.5).abs() < 0.01);
        let (s, a) = f.at(10.135);
        assert!(s == Scene::Lyrics && (a - 0.5).abs() < 0.01);
        assert_eq!(f.at(11.0), (Scene::Lyrics, 1.0));
        assert_eq!(f.toward(Scene::Lyrics, 12.0), f, "the same scene changes nothing");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), ReleaseDay { year: 1970, month: 1, day: Some(1) });
        assert_eq!(civil(11_016), ReleaseDay { year: 2000, month: 2, day: Some(29) });
        assert_eq!(civil(20_766), ReleaseDay { year: 2026, month: 11, day: Some(9) });
    }

    #[test]
    fn ids_of_the_copy_are_its_own() {
        let copy = Stage::copy(0.5, true, (0.0, Vec::new()));
        assert_eq!(Stage::REAL.id("np-mega"), Id::new("np-mega"));
        assert_ne!(copy.id("np-mega"), Id::new("np-mega"));
        assert!(!copy.live && Stage::REAL.live);
        // Only the copy reports its parts.
        let ctx = Context::default();
        Stage::REAL.part(&ctx, Part::Cover, Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0)));
        assert!(ctx.data(|d| d.get_temp::<Parts>(parts_id())).is_none());
        copy.part(&ctx, Part::Cover, Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0)));
        assert_eq!(ctx.data(|d| d.get_temp::<Parts>(parts_id())).map(|p| p.0.len()), Some(1));
    }
}
