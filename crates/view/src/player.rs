//! The player, the queue, the lyrics and the aligner, and a track's
//! technical details.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Stopped,
    Playing,
    Paused,
    /// Waiting for the decoder or a stream.
    Buffering,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

/// Playback as of `at`; the UI extrapolates while playing so progress moves
/// smoothly without a state update per frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub status: Status,
    pub current: Option<TrackRow>,
    pub position: Duration,
    pub at: Instant,
    /// How fast the position moves from `at` while playing: 1, or a little
    /// off while the runtime brings it to what is heard (never below 0).
    /// For a track played at another speed it is that speed, or a little
    /// off: `position` and `duration` are the track's own time.
    pub rate: f32,
    /// The speed the current track plays at (1: as recorded). What the
    /// listener sees is [`heard_time`] of the position and the length.
    pub speed: f32,
    pub duration: Option<Duration>,
    /// 0..1 perceptual.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// What the current queue was started from.
    pub origin: QueueOrigin,
    /// The page of what the queue was started from, when it has one (an
    /// album, a playlist, an artist): the name in the header line opens it.
    pub context_page: Option<Route>,
    /// Pre-rendered Now Playing header line in the active locale.
    pub context_text: String,
    /// Byte range of the clickable entity name inside [`Self::context_text`].
    pub context_link_range: Option<std::ops::Range<usize>>,
    /// The service the current track's sound comes from (`None` for a local
    /// file): the "звук: YouTube" label, e.g. for a Spotify track played
    /// from YouTube Music because Spotify has no sound for this account.
    pub audio_service: Option<Service>,
    /// What the sound of a stream actually is ("Opus ~160 кбит/с", "AAC
    /// ~128 кбит/с", "Vorbis 320 кбит/с"): the source that plays, not the
    /// track's best source. `None` for a local file (its row says).
    pub audio_quality: Option<String>,
    /// The track plays through Spotify Soloist (a live stream of another
    /// player: crossfade and gapless do not apply).
    pub soloist: bool,
    /// «Таймер сна», while one runs.
    pub sleep: Option<SleepView>,
    /// The current duck, if any.
    pub duck: Option<DuckView>,
}

/// The sound lowered for a while ([`Command::SetDuck`]): a voice assistant
/// listens or speaks. The volume stays as it is.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DuckView {
    pub gain: f32,
    /// When it ends by itself (unix ms).
    pub until_unix_ms: i64,
}

impl Default for PlayerView {
    fn default() -> Self {
        Self {
            status: Status::Stopped,
            current: None,
            position: Duration::ZERO,
            at: Instant::now(),
            rate: 1.0,
            speed: 1.0,
            duration: None,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            origin: QueueOrigin::None,
            context_page: None,
            context_text: String::new(),
            context_link_range: None,
            audio_service: None,
            soloist: false,
            audio_quality: None,
            sleep: None,
            duck: None,
        }
    }
}

impl PlayerView {
    /// Updates `origin`, `context_page`, and the pre-rendered header cache in `loc`.
    pub fn set_context(&mut self, origin: QueueOrigin, page: Option<Route>, loc: Locale) {
        self.origin = origin;
        self.context_page = page;
        self.sync_context(loc);
    }

    /// Recomputes [`Self::context_text`] and [`Self::context_link_range`] in `loc`.
    pub fn sync_context(&mut self, loc: Locale) {
        let (text, range) = self.origin.header_in(loc, self.context_page.is_some());
        self.context_text = text;
        self.context_link_range = range;
    }

    /// Zero-allocation access to the pre-rendered header line and optional link span.
    pub fn cached_context(&self) -> (&str, Option<std::ops::Range<usize>>) {
        (&self.context_text, self.context_link_range.clone())
    }

    /// The header line of Now Playing and the exact byte range of the
    /// clickable entity name when [`Self::context_page`] is present.
    pub fn context_header(&self, locale: Locale) -> (String, Option<std::ops::Range<usize>>) {
        self.origin.header_in(locale, self.context_page.is_some())
    }

    /// The header line of Now Playing in the active process locale.
    pub fn context_line(&self) -> String {
        self.context_header(nikgob_core::i18n::locale()).0
    }

    /// Byte range of the clickable name in `line`, when the queue has a page
    /// to open ([`Self::context_page`]).
    pub fn context_link(&self, line: &str) -> Option<std::ops::Range<usize>> {
        let (expected, range) = self.context_header(nikgob_core::i18n::locale());
        (line == expected).then_some(range).flatten()
    }

    /// Position now, extrapolated from the last update.
    pub fn position_now(&self) -> Duration {
        let p = if self.status == Status::Playing {
            self.position + self.at.elapsed().mul_f32(self.rate.max(0.0))
        } else {
            self.position
        };
        match self.duration {
            Some(d) => p.min(d),
            None => p,
        }
    }

    pub fn fraction_now(&self) -> f32 {
        match self.duration {
            Some(d) if !d.is_zero() => {
                (self.position_now().as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueView {
    /// What played before the current track (the last few, oldest first);
    /// "previous" walks back through these.
    pub prev: Vec<QueueEntry>,
    pub current: Option<TrackRow>,
    /// The current track's queue entry id (a drop target in the queue).
    pub current_entry: Option<u64>,
    /// Why the current track was picked, in its row's tooltip (see
    /// [`QueueEntry::why`]).
    pub current_why: Option<String>,
    /// The current track is a guest of «✦ С дополнением».
    pub current_guest: bool,
    /// Upcoming tracks in play order; `entry` ids are stable for drag & drop.
    pub next: Vec<QueueEntry>,
    /// The queue is a radio or an endless mix: its title («Радио · Кино»)
    /// and whether it is looking for more right now. Any other playback
    /// ends it.
    pub radio: Option<RadioView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub entry: u64,
    pub track: TrackRow,
    /// Why a radio or a mix picked it, in its row's tooltip:
    /// the first pick of a radio and the new ones only.
    pub why: Option<String>,
    /// A guest of «✦ С дополнением» (marked ✦ in its row).
    pub guest: bool,
}

/// Synced lyrics for the current track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LyricsView {
    pub track: Option<TrackId>,
    pub lines: Vec<LyricLine>,
    /// Unsynced fallback text.
    pub plain: Option<String>,
    pub loading: bool,
    /// Where the shown text comes from.
    pub source: Option<LyricsSource>,
    /// The source was picked by the user (not chosen automatically).
    pub picked: bool,
    /// Every source that has, or may still get, a text for this track.
    pub sources: Vec<LyricsOption>,
    /// The shown timing was made by the player from the recording, not
    /// given by the source.
    pub made: bool,
    /// The texts are being checked against the recording right now.
    pub checking: bool,
    /// Every text there is, in full («Текст песни» sheet).
    pub texts: Vec<LyricsText>,
    /// Where the shown text is kept.
    pub home: LyricsHome,
    /// Whether timing can be made for the shown text, or why not.
    pub craft: LyricsCraft,
    /// The shown text (with the timing the player made) can be saved on
    /// the computer: into the file's tags and a `.ttml` next to it, or for
    /// a song without a file into its artist's folder of the library.
    pub savable: bool,
    /// How the shown text can be seen, finest first: [`LyricsSync::Word`]
    /// and [`LyricsSync::Line`] as far as it has that timing (the player's
    /// own counts), [`LyricsSync::Plain`] always. Empty without a text.
    pub shows: Vec<LyricsSync>,
    /// The shown track has no file (it plays from a stream): its text on
    /// the computer is one kept in its artist's folder, and «Сохранить
    /// текст» keeps what «Что сохранять» says.
    pub stream: bool,
}

/// Where a shown text is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LyricsHome {
    /// Found online (and cached), nothing of it on the computer.
    #[default]
    Online,
    /// Timed or heard by the player and kept in the library.
    Library,
    /// On the computer: in the file's tags or in a file next to the song.
    Computer,
}

/// Whether timing can be made for the shown text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LyricsCraft {
    /// Not offered (no text, a heard one, or no audio to listen to).
    #[default]
    Off,
    /// From the file, or from the stream for a while.
    Ready,
    /// Not for this song, and why (a clip whose song is unclear).
    No(String),
}

/// One source's text in full, and what the player made of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LyricsText {
    pub source: Option<LyricsSource>,
    /// As the source has it (line starts zero for a plain text).
    pub lines: Vec<LyricLine>,
    pub plain: Option<String>,
    /// The source's own precision.
    pub sync: Option<LyricsSync>,
    /// Checked against the recording: `Some(false)` is another song's or
    /// another version's text. `None`: not checked.
    pub fits: Option<bool>,
    /// Word timing the player made from it (when it fits).
    pub made: Option<Vec<LyricLine>>,
    /// That timing was made and kept by the user.
    pub saved: bool,
}

/// One source in the lyrics source picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsOption {
    pub source: LyricsSource,
    pub state: LyricsAvail,
    /// Checked against the recording (`None`: not yet).
    pub fits: Option<bool>,
    /// Shown with word timing the player made.
    pub made: bool,
}

/// The lyrics models and the work of the aligner (Settings → «Текст песен»,
/// the «Текст песни» sheet).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlignView {
    pub models: Vec<AlignModel>,
    /// What the aligner is doing now.
    pub job: Option<AlignJob>,
    /// The background pass over the library: tracks done of all.
    pub done: u32,
    pub total: u32,
    /// Where the models ran last (`None` before they ran): true the GPU.
    pub on_gpu: Option<bool>,
    /// Timing the user asked for (the lyrics control): under way, or why
    /// it failed. Made timing is applied at once and the craft is gone.
    pub craft: Option<Craft>,
    /// The last thing that went wrong (a download, the runtime).
    pub error: Option<String>,
}

/// One downloadable part (the runtime or a model).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlignModel {
    pub key: String,
    pub label: String,
    /// Bytes to download.
    pub size: u64,
    pub state: ModelState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ModelState {
    #[default]
    Missing,
    /// Downloading (done share).
    Downloading(f32),
    Ready,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlignJob {
    pub track: Option<TrackId>,
    pub stage: AlignStage,
    /// Done share of the stage.
    pub progress: f32,
    /// Asked by the user (the lyrics control), not the background.
    pub asked: bool,
    /// While models download: bytes done and bytes to fetch in all.
    pub bytes: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignStage {
    /// Fetching a model first.
    Download,
    Decode,
    /// The voice out of the music.
    Separate,
    /// The aligner hears the voice.
    Listen,
    Align,
    /// Hearing a text from scratch.
    Recognise,
}

/// Timing the user asked for from one source's text.
#[derive(Clone, Debug, PartialEq)]
pub struct Craft {
    pub track: TrackId,
    pub source: LyricsSource,
    /// [`LyricsSync::Line`] or [`LyricsSync::Word`].
    pub target: LyricsSync,
    /// The result, once made.
    pub lines: Option<Vec<LyricLine>>,
    /// It could not be made (the models, the file).
    pub failed: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LyricsAvail {
    Loading,
    /// Looked up, nothing there.
    Missing,
    Ready(LyricsSync),
}

/// Technical details of a track: the playing one ("Инфо" tab) or one
/// asked for ([`ViewState::track_info`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub track: Option<TrackId>,
    pub quality: Option<String>,
    pub source: Option<String>,
    pub path: Option<PathBuf>,
    pub tags: Vec<(String, String)>,
    /// The «Качество» section under the rest.
    pub sound: Option<SoundDetails>,
}

/// The «Качество» section of a track's info: the technical facts of what
/// plays and what the spectrum check found about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundDetails {
    /// `(name, value)` lines: «Кодек», «Контейнер», «Битрейт»…
    pub facts: Vec<(String, String)>,
    /// The check in plain words; `None` while the file is not checked (or
    /// for a stream, which is not).
    pub verdict: Option<String>,
    /// The sound is less than its format promises (a fake lossless file,
    /// an inflated bitrate).
    pub warning: bool,
    /// The real band: «до 16,0 кГц».
    pub band: Option<String>,
    pub spectrum: Option<SpectrumPlot>,
    /// A file not checked yet: the background check comes to it.
    pub pending: bool,
    /// The file is lossless with an MP3 inside, surely: «Превратить в MP3».
    pub to_mp3: bool,
    /// A file a better copy may be looked for: «Найти качество лучше».
    pub better: bool,
}

/// The averaged spectrum the check judged, for a simple chart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpectrumPlot {
    /// dB re a full-scale sine, bands evenly spread from 0 to `nyquist_hz`.
    pub db: Vec<f32>,
    pub nyquist_hz: f32,
    /// Where the content stops, when it stops before `nyquist_hz`.
    pub cutoff_hz: Option<f32>,
}
