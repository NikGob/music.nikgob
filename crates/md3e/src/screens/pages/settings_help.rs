//! What every settings row is: an id other screens can link to ("настроить"
//! opens Settings on that row, scrolls to it and makes it glow), a one-line
//! explanation and, for the looks, a small live preview painted by the real
//! painters. Both show in a card while the row is hovered.
//!
//! A new row opts in by getting a [`SettingId`] variant, a [`SettingId::tab`]
//! and a [`SettingId::help`] line; [`super::settings`] does the rest.

use egui::{Align, Color32, Frame, Id, Margin, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use nikgob_view::{BgShapes, Prefs};

use crate::{Theme, TypeRole, components::anim, components::slider::wave_line, tokens::shape};
use crate::screens::{bg_shapes, lyrics, now_playing};

/// One settings row (or block of fields).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingId {
    // Внешний вид.
    Design,
    Theme,
    Contrast,
    Scheme,
    ProgressWave,
    NpVolume,
    CoverShape,
    ShapeRange,
    ShapePick,
    CoverSpin,
    CoverDir,
    CoverPulse,
    Ring,
    RingStyle,
    RingSpin,
    RingDir,
    RingBeat,
    Ambient,
    BgShapes,
    BgCount,
    BgSize,
    BgOpacity,
    BgSpeed,
    BgReact,
    BgColors,
    LyricsStyle,
    WaveFallback,
    WaveJump,
    LyricsDim,
    LyricsScale,
    LyricsCascade,
    LyricsGlow,
    LyricsGap,
    LyricsGapWaves,
    LyricsCountdown,
    NavHidden,
    NpMotion,
    ReducedMotion,
    FpsLimit,
    // Основные.
    CoverClickPause,
    DblClickShape,
    Keys,
    SaveLyrics,
    LyricsReturn,
    OutputDevice,
    Gapless,
    Crossfade,
    Normalization,
    LibraryDir,
    MastersDir,
    Inbox,
    NamingRules,
    Trash,
    ExportData,
    OpusBitrate,
    FakeFlac,
    MusicBrainz,
    AcoustId,
    Prowlarr,
    NoSeed,
    SeedLimit,
    SeedDays,
    SeedRatio,
    DownloadLimit,
    UploadLimit,
    Spotify,
    YouTube,
    Nodes,
}

/// Settings tabs, in the order of the tab switch.
pub const TAB_GENERAL: usize = 0;
pub const TAB_APPEARANCE: usize = 1;

/// The live sample a hover card draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Preview {
    Palette,
    Ambient,
    Wave,
    Cover,
    Ring,
    Background,
    Lyrics,
    Gap,
}

impl SettingId {
    /// The tab the row lives on.
    pub fn tab(self) -> usize {
        use SettingId::*;
        match self {
            Design | Theme | Contrast | Scheme | ProgressWave | NpVolume | CoverShape | ShapeRange | ShapePick
            | CoverSpin | CoverDir | CoverPulse | Ring | RingStyle | RingSpin | RingDir | RingBeat | Ambient
            | BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors | LyricsStyle | WaveFallback
            | WaveJump | LyricsDim | LyricsScale | LyricsCascade | LyricsGlow | LyricsGap | LyricsGapWaves
            | LyricsCountdown | NavHidden | NpMotion | ReducedMotion | FpsLimit => TAB_APPEARANCE,
            _ => TAB_GENERAL,
        }
    }

    /// One line on what the option changes.
    pub fn help(self) -> &'static str {
        use SettingId::*;
        match self {
            Design => "Общий стиль интерфейса. Пока есть только Material 3 Expressive.",
            Theme => "Светлая, тёмная или как в системе.",
            Contrast => "Насколько текст и элементы отличаются от фона.",
            Scheme => "Как из обложки трека получается палитра: от спокойной до яркой.",
            ProgressWave => "Сыгранная часть полосы прогресса бежит волной, пока играет музыка.",
            NpVolume => "Ползунок громкости в открытом треке. Без него — кнопка звука и колесо мыши.",
            CoverShape => "Квадратная обложка, одна фигура Material или случайная на каждый трек.",
            ShapeRange => "Набор фигур, из которых выбирается случайная. Клик — добавить или убрать.",
            ShapePick => "Какой фигурой обрезана обложка, пока играет музыка.",
            CoverSpin => "Контур обложки медленно крутится, сама картинка стоит.",
            CoverDir => "В какую сторону крутится контур обложки.",
            CoverPulse => "Обложка чуть увеличивается на каждой доле.",
            Ring => "Точки или бары вокруг обложки, которые двигаются под музыку.",
            RingStyle => "Круг из точек, точки по контуру фигуры или бары-спектр.",
            RingSpin => "Кольцо медленно вращается, пока играет музыка.",
            RingDir => "В какую сторону вращается кольцо.",
            RingBeat => "Точки отскакивают на ударных и там, где громче спектр.",
            Ambient => "Мягкие цветные облака за плеером; «Под звук» — дышат с басом.",
            BgShapes => "Фигуры Material за открытым треком: прыгают, парят или меняют форму.",
            BgCount => "Сколько фигур на фоне.",
            BgSize => "Размер фигур на фоне.",
            BgOpacity => "Насколько фигуры заметны на фоне.",
            BgSpeed => "Как быстро фигуры плывут, вращаются и прыгают.",
            BgReact => "Насколько сильно фигуры отзываются на музыку.",
            BgColors => "Цвета фигур: контейнеры темы, один акцент или все яркие акценты.",
            LyricsStyle => "Как подсвечивается строка, которую сейчас поют.",
            WaveFallback => "Что показывать вместо «Волны», если у текста нет разметки по слогам.",
            WaveJump => "Как высоко подпрыгивают буквы в анимации «Волна».",
            LyricsDim => "Насколько приглушены строки, которые сейчас не поют.",
            LyricsScale => "Размер шрифта в тексте песен.",
            LyricsCascade => "Дальние строки догоняют прокрутку с задержкой, как каскад.",
            LyricsGlow => "Спетая часть строки слегка светится.",
            LyricsGap => "Как выглядит индикатор паузы между строками.",
            LyricsGapWaves => "Сколько гребней у волнистого индикатора паузы.",
            LyricsCountdown => "В последние секунды паузы индикатор считает 3-2-1.",
            NavHidden => "Прячет левую панель вместе с кнопкой раскрытия; она выезжает у левого края окна.",
            NpMotion => "Скорость и характер анимации, с которой открывается экран трека.",
            ReducedMotion => "Убирает отскоки, вращение и волны во всём интерфейсе.",
            FpsLimit => "Максимум кадров в секунду. Меньше — тише вентилятор и дольше батарея.",
            CoverClickPause => "Клик по обложке в открытом треке ставит на паузу и снимает с неё.",
            DblClickShape => "Двойной клик в открытом треке меняет фигуру обложки.",
            Keys => "Горячие клавиши в открытом треке.",
            SaveLyrics => "Найденный в сети текст записывается в теги файла и работает без интернета.",
            LyricsReturn => "Через сколько секунд после ручной прокрутки текст вернётся к текущей строке.",
            OutputDevice => "Куда идёт звук. Переключается без паузы.",
            Gapless => "Треки альбома играют встык, без тишины между ними.",
            Crossfade => "Конец трека плавно перетекает в начало следующего.",
            Normalization => "Выравнивает громкость треков по ReplayGain / R128.",
            LibraryDir => "Где лежит музыка библиотеки.",
            MastersDir => "Куда складываются исходные FLAC после перекодирования в Opus.",
            Inbox => "Папка, из которой новые файлы импортируются в библиотеку.",
            NamingRules => "Как называются папки и файлы после импорта.",
            Trash => "Удалённые треки сначала попадают сюда и лежат 30 дней.",
            ExportData => "Лайки и плейлисты лежат файлами рядом с музыкой.",
            OpusBitrate => "Качество Opus при импорте: выше — лучше звук и больше файлы.",
            FakeFlac => "Ищет FLAC, перекодированные из MP3: у них обрезан спектр.",
            MusicBrainz => "Контакт, который MusicBrainz просит указывать в запросах.",
            AcoustId => "Ключ для опознания треков по звуку.",
            Prowlarr => "Адрес и ключ Prowlarr — через него идёт поиск раздач.",
            NoSeed => "Скачанное сразу перестаёт раздаваться.",
            SeedLimit => "Сколько всего можно отдать, прежде чем раздачи остановятся.",
            SeedDays => "Сколько дней держать раздачу, потом чистка.",
            SeedRatio => "Раздача останавливается, когда отдано в столько раз больше скачанного.",
            DownloadLimit => "Ограничение скорости загрузки торрентов.",
            UploadLimit => "Ограничение скорости отдачи торрентов.",
            Spotify => "Импорт лайков и плейлистов из Spotify — появится позже.",
            YouTube => "Импорт из YouTube Music — появится позже.",
            Nodes => "Синхронизация и бэкап на другие устройства — появится позже.",
        }
    }

    fn preview(self) -> Option<Preview> {
        use SettingId::*;
        Some(match self {
            Theme | Contrast | Scheme => Preview::Palette,
            Ambient => Preview::Ambient,
            ProgressWave | ReducedMotion => Preview::Wave,
            CoverShape | ShapeRange | ShapePick | CoverSpin | CoverDir | CoverPulse => Preview::Cover,
            Ring | RingStyle | RingSpin | RingDir | RingBeat => Preview::Ring,
            BgShapes | BgCount | BgSize | BgOpacity | BgSpeed | BgReact | BgColors => Preview::Background,
            LyricsStyle | WaveFallback | WaveJump | LyricsDim | LyricsScale | LyricsGlow => Preview::Lyrics,
            LyricsGap | LyricsGapWaves | LyricsCountdown => Preview::Gap,
            _ => return None,
        })
    }
}

/// How long the glow on a linked row lasts.
pub const FOCUS_S: f64 = 1.5;
/// Frames the scroll to a linked row is repeated, while the page settles.
const SCROLL_FRAMES: u8 = 3;

/// The row a link opened, kept for its glow.
#[derive(Clone, Copy, Debug)]
struct Focus {
    id: SettingId,
    start: f64,
    scrolls: u8,
    rect: Option<Rect>,
}

/// The row under the pointer this frame.
#[derive(Clone, Debug)]
struct Hover {
    id: SettingId,
    label: String,
    rect: Rect,
    pass: u64,
}

fn focus_key() -> Id {
    Id::new("settings-focus")
}

fn hover_key() -> Id {
    Id::new("settings-hover")
}

/// Starts the scroll and glow for `id` (the page then switches its tab).
pub fn begin_focus(ctx: &egui::Context, id: SettingId) {
    let start = anim::now(ctx);
    ctx.data_mut(|d| d.insert_temp(focus_key(), Focus { id, start, scrolls: SCROLL_FRAMES, rect: None }));
}

/// The glow (0..1) `t` seconds after a link opened the row: two soft
/// pulses fading out by [`FOCUS_S`].
pub fn glow_at(t: f32) -> f32 {
    let total = FOCUS_S as f32;
    if !(0.0..total).contains(&t) {
        return 0.0;
    }
    let rise = (t / 0.15).clamp(0.0, 1.0);
    let fall = 1.0 - (t / total).powi(2);
    let pulse = 0.7 + 0.3 * (t * std::f32::consts::TAU / 0.75).cos();
    rise * fall * pulse
}

/// Wraps one settings item: the glow behind it when a link opened it, and
/// the hover card about it.
pub fn item<R>(ui: &mut Ui, id: SettingId, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let bg = ui.painter().add(Shape::Noop);
    let inner = ui.scope(add);
    let rect = inner.response.rect;
    let ctx = ui.ctx().clone();
    if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key()))
        && f.id == id
    {
        let t = (anim::now(&ctx) - f.start) as f32;
        let a = glow_at(t);
        if a > 0.0 {
            let s = Theme::get(&ctx).scheme;
            // The whole row of the column, with room around the text. It
            // may reach past the page's left edge, so only the vertical clip
            // of the scroll area applies.
            let row = Rect::from_x_y_ranges(ui.max_rect().x_range(), rect.y_range());
            let r = row.expand2(vec2(12.0, 7.0));
            let clip = ui.clip_rect();
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(Rect::from_x_y_ranges(r.x_range(), clip.y_range()));
            painter.set(bg, Shape::rect_filled(r, shape::LARGE, s.primary.gamma_multiply(0.18 * a)));
            painter.rect_stroke(r, shape::LARGE, Stroke::new(2.0, s.primary.gamma_multiply(0.7 * a)), egui::StrokeKind::Inside);
            ctx.request_repaint();
        }
        f.rect = Some(rect);
        ctx.data_mut(|d| d.insert_temp(focus_key(), f));
    }
    if ui.rect_contains_pointer(rect) {
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(hover_key(), Hover { id, label: label.to_owned(), rect, pass }));
    }
    inner.inner
}

/// After the page: scrolls to a linked row and shows the hover card for the
/// row under the pointer, with a preview of `prefs` as they are now.
pub fn finish(ui: &mut Ui, prefs: &Prefs) {
    let ctx = ui.ctx().clone();
    if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key())) {
        if let Some(rect) = f.rect.take()
            && f.scrolls > 0
        {
            // After the controls, so their nested scroll areas cannot take it.
            ui.scroll_to_rect(rect.expand(24.0), Some(Align::Center));
            f.scrolls -= 1;
            ctx.request_repaint();
        }
        if f.scrolls == 0 && anim::now(&ctx) - f.start > FOCUS_S {
            ctx.data_mut(|d| d.remove::<Focus>(focus_key()));
        } else {
            ctx.data_mut(|d| d.insert_temp(focus_key(), f));
        }
    }

    let Some(h) = ctx.data(|d| d.get_temp::<Hover>(hover_key())) else { return };
    if h.pass != ctx.cumulative_pass_nr() {
        return;
    }
    let resp = ui.interact(h.rect, Id::new(("settings-help", h.id)), Sense::hover());
    let s = Theme::get(&ctx).scheme;
    let frame = Frame {
        inner_margin: Margin::same(12),
        outer_margin: Margin::ZERO,
        corner_radius: egui::CornerRadius::same(shape::MEDIUM as u8),
        shadow: egui::epaint::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(40) },
        fill: s.surface_container,
        stroke: Stroke::NONE,
    };
    let mut tip = egui::Tooltip::for_enabled(&resp);
    tip.popup = tip
        .popup
        .frame(frame)
        .width(CARD_W)
        .align(egui::RectAlign::BOTTOM_END)
        .align_alternatives(&[egui::RectAlign::TOP_END, egui::RectAlign::RIGHT_START]);
    tip.show(|ui| {
        ui.set_width(CARD_W);
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some(kind) = h.id.preview() {
            let (rect, _) = ui.allocate_exact_size(vec2(CARD_W, PREVIEW_H), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, shape::SMALL, s.surface);
            let mut pui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            pui.set_clip_rect(rect.intersect(ui.clip_rect()));
            paint_preview(&mut pui, rect, kind, prefs);
            ui.add_space(4.0);
        }
        ui.label(TypeRole::TitleSmall.style().rich(h.label.as_str(), s.on_surface));
        ui.label(TypeRole::BodyMedium.style().rich_para(h.id.help(), s.on_surface_variant));
    });
}

const CARD_W: f32 = 280.0;
const PREVIEW_H: f32 = 132.0;

fn paint_preview(ui: &mut Ui, rect: Rect, kind: Preview, prefs: &Prefs) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    match kind {
        Preview::Palette => {
            // The scheme the interface paints with right now.
            let swatches = [
                (s.primary, "primary"),
                (s.secondary, "secondary"),
                (s.tertiary, "tertiary"),
                (s.primary_container, "контейнер"),
                (s.surface_container_highest, "поверхность"),
            ];
            let w = (rect.width() - 16.0) / swatches.len() as f32;
            for (i, (c, _)) in swatches.iter().enumerate() {
                let r = Rect::from_min_size(pos2(rect.left() + 8.0 + i as f32 * w, rect.top() + 12.0), vec2(w - 6.0, 64.0));
                ui.painter().rect_filled(r, shape::MEDIUM, *c);
            }
            let text = Rect::from_min_size(pos2(rect.left() + 12.0, rect.top() + 88.0), vec2(rect.width() - 24.0, 32.0));
            ui.painter().rect_filled(text, 16.0, s.primary);
            ui.painter().text(text.center(), egui::Align2::CENTER_CENTER, "Кнопка", TypeRole::LabelLarge.style().font_id(), s.on_primary);
        }
        Preview::Ambient => {
            // Off: a sample of the calm glow.
            let t = anim::now(ui.ctx()) as f32;
            let bass = if prefs.ambient == nikgob_view::Ambient::Reactive { beat(t) } else { 0.0 };
            crate::screens::ambient::paint(ui.ctx(), ui.painter(), rect, bass, true);
        }
        Preview::Wave => {
            let t = anim::now(ui.ctx()) as f32;
            let cy = rect.center().y;
            let (x0, x1) = (rect.left() + 20.0, rect.right() - 20.0);
            let head = x0 + (x1 - x0) * (0.25 + 0.5 * ((t * 0.1).fract()));
            let wavy = prefs.progress_wave && !theme.reduced_motion;
            let amp = if wavy { 3.0 } else { 0.0 };
            let phase = (t * 0.6).fract() * std::f32::consts::TAU;
            wave_line(ui, x0, head, cy, amp, 40.0, phase, Stroke::new(4.0, s.primary));
            ui.painter().line_segment([pos2(head + 8.0, cy), pos2(x1, cy)], Stroke::new(4.0, s.secondary_container));
            ui.painter().circle_filled(pos2(x1, cy), 2.0, s.primary);
        }
        Preview::Cover => now_playing::preview(ui, rect, prefs, false),
        Preview::Ring => now_playing::preview(ui, rect, prefs, prefs.ring),
        Preview::Background => {
            // Off: a sample of how they would float.
            let sample;
            let prefs = if prefs.bg_shapes == BgShapes::Off {
                sample = Prefs { bg_shapes: BgShapes::Float, ..prefs.clone() };
                &sample
            } else {
                prefs
            };
            let t = anim::now(ui.ctx()) as f32;
            let kick = beat(t);
            let transients: Vec<f32> = (0..8).map(|i| kick * (0.6 + 0.4 * (i as f32 * 1.7 + t).sin())).collect();
            bg_shapes::preview(ui, rect, prefs, kick, &transients);
        }
        Preview::Lyrics => lyrics::preview(ui, rect, prefs),
        Preview::Gap => lyrics::gap_preview(ui, rect, prefs),
    }
    ui.ctx().request_repaint();
}

/// A made-up 120 BPM beat (1 on the beat, decaying).
pub(crate) fn beat(t: f32) -> f32 {
    (-(t * 2.0).fract() * 6.0).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glow_pulses_then_fades_out() {
        assert_eq!(glow_at(-0.1), 0.0);
        assert!(glow_at(0.0) < 0.05);
        assert!(glow_at(0.2) > 0.5);
        // A dip between the two pulses.
        assert!(glow_at(0.375) < glow_at(0.75));
        assert!(glow_at(1.4) < 0.2);
        assert_eq!(glow_at(FOCUS_S as f32), 0.0);
    }

    #[test]
    fn linked_rows_know_their_tab_and_every_row_has_help() {
        assert_eq!(SettingId::LyricsStyle.tab(), TAB_APPEARANCE);
        assert_eq!(SettingId::Prowlarr.tab(), TAB_GENERAL);
        assert_eq!(SettingId::NoSeed.tab(), TAB_GENERAL);
        for id in [SettingId::Design, SettingId::FpsLimit, SettingId::Nodes, SettingId::RingBeat] {
            assert!(!id.help().is_empty());
        }
    }
}
