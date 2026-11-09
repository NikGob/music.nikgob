//! The view model every design system draws from.
//!
//! The runtime (library, player, import, downloads) fills a [`ViewState`];
//! a design system reads it each frame and answers with [`Command`]s. Neither
//! side knows how the other works: MD3E, Nothing and Liquid Glass are
//! different layouts over the same state.

use std::path::PathBuf;
use std::time::{Duration, Instant};

mod credit;
mod recs;
mod streams;

pub use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource, LyricsSync};
pub use nikgob_core::model::{
    ArtistId, LikeTarget, PlaylistId, ReleaseId, ReleaseType, SourceKind, TrackId,
};
pub use credit::*;
pub use recs::*;
pub use streams::*;

/// A cover image on disk plus the palette seed computed at import.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cover {
    pub path: PathBuf,
    /// ARGB seed for the colour scheme, if the cover is colourful enough.
    pub seed: Option<u32>,
    /// A tiny (16×16) downscale of the cover; stretched with linear
    /// filtering it is the blurred ambient background.
    pub ambient: Option<PathBuf>,
}

impl Cover {
    /// A cover of a streaming service, drawn from its `https://` url (the
    /// app's web image loader fetches it; a placeholder while it cannot).
    pub fn web(url: impl Into<String>) -> Self {
        Self {
            path: PathBuf::from(url.into()),
            seed: None,
            ambient: None,
        }
    }

    /// The url of a [`Self::web`] cover.
    pub fn web_url(&self) -> Option<&str> {
        self.path
            .to_str()
            .filter(|p| p.starts_with("https://") || p.starts_with("http://"))
    }

    /// The cover file on disk (`None` for a web cover).
    pub fn file(&self) -> Option<&std::path::Path> {
        self.web_url().is_none().then_some(self.path.as_path())
    }

    /// The egui image URI for this file (a web cover: its url).
    pub fn uri(&self) -> String {
        match self.web_url() {
            Some(url) => url.to_owned(),
            None => file_uri(&self.path),
        }
    }

    pub fn ambient_uri(&self) -> Option<String> {
        self.ambient.as_deref().map(file_uri)
    }
}

/// `file://` URI egui's file loader understands (forward slashes on Windows).
pub fn file_uri(path: &std::path::Path) -> String {
    let p = path.display().to_string().replace('\\', "/");
    // egui_extras wants `file:///C:/...` on Windows (empty host).
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// Live audio levels for ambient effects, read by the UI only while visible.
pub trait Levels {
    /// Bass energy 20–150 Hz, smoothed, 0..1.
    fn bass(&self) -> f32;
    /// Log-spaced spectrum bands, 0..1 each; fills `out`.
    fn bands(&self, out: &mut [f32]);
}

/// No audio analysis available.
pub struct NoLevels;

impl Levels for NoLevels {
    fn bass(&self) -> f32 {
        0.0
    }
    fn bands(&self, out: &mut [f32]) {
        out.fill(0.0);
    }
}

/// One row of a track list.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackRow {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub release_id: Option<ReleaseId>,
    /// Every credited artist (a link on each name of `artist`).
    pub artists: ArtistLinks,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration: Option<Duration>,
    /// e.g. "FLAC → Opus 160", "MP3 320".
    pub quality: Option<String>,
    /// Where it would play from right now; `None` = nothing playable.
    pub source: Option<SourceKind>,
    pub has_local: bool,
    pub liked: bool,
    pub cover: Option<Cover>,
    /// What plays is a YouTube music video, not the song («· клип»).
    pub clip: bool,
}

impl TrackRow {
    /// The first (main) credited artist.
    pub fn artist_id(&self) -> Option<ArtistId> {
        self.artists.main().copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseCard {
    pub id: ReleaseId,
    pub title: String,
    pub artist: String,
    /// Every credited artist (a link on each name of `artist`).
    pub artists: ArtistLinks,
    pub year: Option<i32>,
    pub release_type: ReleaseType,
    pub cover: Option<Cover>,
    pub local_tracks: u32,
    pub total_tracks: Option<u32>,
    pub liked: bool,
    /// When it came into the library (Unix ms): «Недавно добавлено» and the
    /// group headers of that sort.
    pub added_at: i64,
}

impl ReleaseCard {
    /// The first (main) credited artist.
    pub fn artist_id(&self) -> Option<ArtistId> {
        self.artists.main().copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseDetail {
    pub card: ReleaseCard,
    pub tracks: Vec<TrackRow>,
    pub total_duration: Duration,
    /// "FLAC → Opus" style summary chip.
    pub quality: Option<String>,
    pub liked: bool,
    /// The page's lines in order, one per place the album counts (the
    /// card's «13 из 14» is their count); empty: just the tracks.
    pub lines: Vec<ReleaseLine>,
    /// The day (or month) it came out, when known beyond the card's year:
    /// from its files' tags, else from a service.
    pub released: Option<ReleaseDay>,
    /// The record label, when its files' tags name one.
    pub label: Option<String>,
}

/// The day an album came out, or its month when the day is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseDay {
    pub year: i32,
    /// 1–12.
    pub month: u8,
    /// 1–31.
    pub day: Option<u8>,
}

impl ReleaseDay {
    /// From a tag's or a service's date: `2019-05-17`, `2019-05`,
    /// `2019-05-17T00:00:00Z`, `2019/05/17`. `None` for a bare year,
    /// anything else, or a date in another year than `year` (when known: the
    /// date of a recording is not the release's).
    pub fn parse(date: &str, year: Option<i32>) -> Option<Self> {
        let date = date.trim();
        let date = date.split(['T', ' ']).next().unwrap_or(date);
        let mut parts = date.split(['-', '/', '.']);
        let y = parts.next().filter(|p| p.len() == 4)?.parse::<i32>().ok()?;
        let m = parts.next().filter(|p| (1..=2).contains(&p.len()))?.parse::<u8>().ok()?;
        let d = match parts.next() {
            Some(p) if (1..=2).contains(&p.len()) => Some(p.parse::<u8>().ok()?),
            Some(_) => return None,
            None => None,
        };
        if parts.next().is_some() || !(1000..=9999).contains(&y) || !(1..=12).contains(&m) {
            return None;
        }
        if d.is_some_and(|d| !(1..=31).contains(&d)) || year.is_some_and(|year| year != y) {
            return None;
        }
        Some(Self { year: y, month: m, day: d })
    }
}

/// One line of an album page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseLine {
    /// A track of [`ReleaseDetail::tracks`], by index.
    Track(usize),
    /// A place the album counts that has no track: «Трек 7 · нет файла»
    /// (`no` 0 when unknown).
    Hole { disc: u32, no: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistCard {
    pub id: ArtistId,
    pub name: String,
    pub image: Option<Cover>,
    /// "Подписан" (liked).
    pub liked: bool,
    /// Their tracks in the library («12 треков» under the avatar); 0 when
    /// not counted.
    pub track_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistDetail {
    pub card: ArtistCard,
    pub top_tracks: Vec<TrackRow>,
    pub albums: Vec<ReleaseCard>,
    pub singles_eps: Vec<ReleaseCard>,
    pub liked: bool,
    /// "Нет в библиотеке": the artist's albums in the services that the
    /// library has no file of ([`Command::LoadDiscography`]).
    pub discography: DiscographyView,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistCard {
    pub id: PlaylistId,
    pub name: String,
    /// What to show: the custom picture, the 2x2 collage or the most common
    /// album cover, or a generated placeholder (see [`PlaylistCoverMode`]).
    pub cover: Option<Cover>,
    pub track_count: u32,
    pub pinned: bool,
    pub cover_mode: PlaylistCoverMode,
    /// The service whose playlist it follows (or is synced with), if any.
    pub service: Option<Service>,
}

/// How a playlist's cover is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlaylistCoverMode {
    /// A collage when the tracks have 4+ different album covers, else the most
    /// common cover, else a generated gradient.
    #[default]
    Smart,
    /// The first 4 different album covers, 2x2.
    Collage,
    /// A picture the user picked.
    Custom,
}

/// One day of «Скачано».
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadedDay {
    /// «Сегодня», «Вчера», «28 октября».
    pub label: String,
    pub tracks: Vec<TrackRow>,
}

/// A deleted track in the "Корзина" view.
#[derive(Clone, Debug, PartialEq)]
pub struct TrashItem {
    pub track: TrackRow,
    /// "сегодня", "3 дн. назад".
    pub deleted: String,
    /// Days until it is removed for good.
    pub days_left: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistDetail {
    pub card: PlaylistCard,
    pub tracks: Vec<TrackRow>,
    pub total_duration: Duration,
    /// The link to a playlist of a service, if any (the mode for its menu,
    /// [`Command::SetPlaylistSync`]).
    pub sync: Option<PlaylistSyncView>,
}

/// The liked-tracks playlist ("Любимые") is virtual: this id stands for it.
pub const LIKED_PLAYLIST: PlaylistId = PlaylistId(nikgob_core::model::Uuid::nil());

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
    pub rate: f32,
    pub duration: Option<Duration>,
    /// 0..1 perceptual.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// What the current queue was started from ("Играет из «…»").
    pub context: Option<String>,
    /// The page of what the queue was started from, when it has one (an
    /// album, a playlist, an artist): the name in the header line opens it.
    pub context_page: Option<Route>,
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
}

impl Default for PlayerView {
    fn default() -> Self {
        Self {
            status: Status::Stopped,
            current: None,
            position: Duration::ZERO,
            at: Instant::now(),
            rate: 1.0,
            duration: None,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            context: None,
            context_page: None,
            audio_service: None,
            soloist: false,
            audio_quality: None,
        }
    }
}

impl PlayerView {
    /// The header line of Now Playing: "Играет из альбома «…»", "Играет
    /// из поиска Spotify: лёд", "Играет из «Библиотека»" — never quotes in
    /// quotes. A context that is a phrase ("альбома «…»", "поиска: …") is
    /// used as it is, a bare name is quoted; old saved contexts ("Поиск
    /// «…»", "Spotify · «…»") read the new way.
    pub fn context_line(&self) -> String {
        let Some(c) = self.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
            return "Сейчас играет".to_owned();
        };
        let unquote = |s: &str| s.trim().trim_start_matches('«').trim_end_matches('»').to_owned();
        const PHRASES: [&str; 5] = ["поиска", "альбома ", "плейлиста ", "исполнителя ", "микса "];
        if PHRASES.iter().any(|p| c.starts_with(p)) || c == "библиотеки" {
            return format!("Играет из {c}");
        }
        if let Some(q) = c.strip_prefix("Поиск ") {
            return format!("Играет из поиска: {}", unquote(q));
        }
        if let Some((service, q)) = c.split_once(" · ") {
            return format!("Играет из поиска {service}: {}", unquote(q));
        }
        if c.contains('«') || c.contains('»') {
            return format!("Играет из {c}");
        }
        format!("Играет из «{c}»")
    }

    /// Where the name is in `line` ([`Self::context_line`]: between « and
    /// »), when the queue has a page to open ([`Self::context_page`]): the
    /// link of the header line.
    pub fn context_link(&self, line: &str) -> Option<std::ops::Range<usize>> {
        self.context_page.as_ref()?;
        let start = line.find('«')? + '«'.len_utf8();
        let end = line.rfind('»')?;
        (start < end).then_some(start..end)
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
    /// Why a radio or a mix picked it, in its row's tooltip (recs-v2 §12):
    /// the first pick of a radio and the new ones only.
    pub why: Option<String>,
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
    /// The shown timing was made by the player from the recording
    /// (docs/lyrics-align.md), not given by the source.
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

/// Technical details of the current track ("Инфо" tab).
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

/// One card of «Продолжить»: an album of the library, or an album of a
/// service that was streamed without joining the library (it opens the
/// service's page).
#[derive(Clone, Debug, PartialEq)]
pub enum ContinueCard {
    Release(ReleaseCard),
    Stream(StreamAlbumCard),
}

/// A shortcut at the top of Главная: what the user is into now (an album,
/// a playlist or an artist), by plays that fade with a half-life of three
/// days. A click opens it, ▶ plays it.
#[derive(Clone, Debug, PartialEq)]
pub enum Shortcut {
    Release(ReleaseCard),
    /// The playlist and up to four covers of its albums (a 2×2 mosaic).
    Playlist(PlaylistCard, Vec<Cover>),
    Artist(ArtistCard),
}

/// An album of «Из твоей библиотеки» (or «С чего начать» without a
/// history): how many of its tracks were ever played.
#[derive(Clone, Debug, PartialEq)]
pub struct ShelfAlbum {
    pub card: ReleaseCard,
    pub heard: u32,
    pub total: u32,
}

/// A world of the card at the top of Главная («Миры»): a big body of
/// music to travel through and finish — a soundtrack franchise across all
/// its albums, or an artist's whole discography in the library — with how
/// far the user got.
#[derive(Clone, Debug, PartialEq)]
pub struct World {
    /// Stable across builds: the franchise's key, or `artist:` and its id.
    pub key: String,
    /// «MADE IN ABYSS», «Серега Пират».
    pub title: String,
    pub kind: WorldKind,
    /// The album on the stage and on its tile: the one playing, else the
    /// last played, else the first.
    pub release: Option<ReleaseId>,
    pub cover: Option<Cover>,
    /// A larger copy of the stage cover from a service when the album's own
    /// is small for the stage (it replaces it once loaded).
    pub cover_hd: Option<Cover>,
    /// «Слушаешь сейчас», «Слушал вчера», «Ещё не начинал».
    pub kicker: String,
    /// «Саундтрек · Kevin Penkin · 4 альбома · 134 трека» / «Исполнитель ·
    /// 7 релизов · 96 треков».
    pub facts: String,
    /// Tracks of it ever started, of all its tracks: «Ты слышал 36 из 134».
    pub heard: u32,
    pub total: u32,
    /// What its main button does.
    pub start: WorldStart,
    /// One of its tracks plays now.
    pub playing: bool,
    /// The tracks of the main button: «Дослушать» the unheard ones with a
    /// file in album order, «Начать» / «С начала» all from the first track
    /// of the first album.
    pub primary: Vec<TrackId>,
    /// «98 треков, 4 ч 35 мин: что ещё не слышал, по порядку альбомов».
    pub primary_tip: String,
    /// «Вперемешку»: all its tracks, shuffled.
    pub shuffled: Vec<TrackId>,
    /// Its albums in the library, in order.
    pub albums: Vec<ReleaseId>,
    /// The three cards under the stage.
    pub cards: Vec<WorldCard>,
}

/// What a world is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorldKind {
    /// A game's or an anime's music across its albums.
    #[default]
    Soundtrack,
    /// An artist's discography.
    Artist,
}

impl WorldKind {
    /// «Саундтрек», «Исполнитель».
    pub fn label(self) -> &'static str {
        match self {
            Self::Soundtrack => "Саундтрек",
            Self::Artist => "Исполнитель",
        }
    }
}

/// The main button of a world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorldStart {
    /// «Дослушать»: something of it was heard, something is left.
    #[default]
    Finish,
    /// «Начать»: nothing of it was played.
    Begin,
    /// «С начала»: everything with a file was heard.
    Again,
}

impl WorldStart {
    pub fn label(self) -> &'static str {
        match self {
            Self::Finish => "Дослушать",
            Self::Begin => "Начать",
            Self::Again => "С начала",
        }
    }
}

/// An album of a world's page with how much of it was heard.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldAlbum {
    pub card: ReleaseCard,
    pub heard: u32,
    pub total: u32,
}

/// A world's page ([`Route::World`], [`Command::LoadWorld`]).
#[derive(Clone, Debug, PartialEq)]
pub struct WorldView {
    pub world: World,
    /// Its albums in order.
    pub albums: Vec<WorldAlbum>,
    /// «Новое»: its new album of the services, not in the library.
    pub new_album: Option<StreamAlbumCard>,
    /// «Любимое»: its liked tracks (shown from three).
    pub liked: Vec<TrackRow>,
    /// «Миксы»: its mixes (recs-v2 §8.5), none before the recommendations
    /// are built.
    pub mixes: Vec<LaneCard>,
}

/// One of the three cards under a world's stage: one click each.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldCard {
    pub icon: WorldIcon,
    /// «Продолжить», «По порядку», «Новый альбом».
    pub kicker: String,
    /// «OST 1, трек 4».
    pub title: String,
    pub go: WorldGo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldIcon {
    NowPlaying,
    Play,
    Next,
    Album,
    Favorite,
    Sound,
}

/// What a world's card does.
#[derive(Clone, Debug, PartialEq)]
pub enum WorldGo {
    /// The player (what plays now).
    Player,
    /// Plays these tracks from the first.
    Play(Vec<TrackId>),
    /// Opens a library album.
    Release(ReleaseId),
    /// Opens an album of a service.
    Stream(RemoteId),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeView {
    /// Главная is not built yet (the start of a run): the page shows the
    /// app's mark instead of an empty library.
    pub loading: bool,
    pub greeting_hour: u8,
    /// Something was ever played, liked or put in a playlist.
    pub history: bool,
    /// The shortcuts at the top (six, eight on a wide window), or the
    /// recently added albums without a history.
    pub shortcuts: Vec<Shortcut>,
    /// «Миксы для тебя»: the moments of the day in the order of now, then
    /// «На повторе».
    pub lanes: Vec<LaneCard>,
    /// The taste profiles «Смешать с…» offers.
    pub blend_profiles: Vec<BlendProfile>,
    /// «Слушать снова»: loved songs not played for a few days.
    pub listen_again: Vec<TrackRow>,
    /// «Из твоей библиотеки»: albums mostly unheard, other ones each day.
    pub from_library: Vec<ShelfAlbum>,
    /// The worlds of the card at the top, the one to open on first.
    pub worlds: Vec<World>,
    /// The library tracks of «Твой микс» (the hero's tracks; it plays with
    /// [`Command::PlayMix`] [`MixId::Daily`]).
    pub mix: Vec<TrackRow>,
    /// «Твой микс»: «30 треков · обновлён сегодня» and four covers. `None`
    /// until the recommendations are built the first time (or with an
    /// empty library).
    pub hero: Option<MixCard>,
    /// «Дейлист» ([`MixId::Daylist`]), the bento's big tile: title «Тягучий
    /// вечер четверга», subtitle «Massive Attack, Kevin Penkin и другие ·
    /// до 23:00», four covers. `None` until the recommendations are built
    /// (or with an empty library).
    pub daylist: Option<MixCard>,
    /// The bento's smaller tiles of the day ([`MixId::Tile`]): «На повторе»
    /// first when it has enough songs, then a few other kinds that change
    /// every day (kicker «ИСПОЛНИТЕЛЬ ДНЯ», title «Kevin Penkin», subtitle
    /// «24 трека · лучшее»).
    pub tiles: Vec<MixCard>,
    /// «Под настроение»: the six moods, in the order for the part of the
    /// day (night: Спокойное, Мрачное first). Empty without any sound
    /// analysis.
    pub moods: Vec<MixCard>,
    /// «Твои миксы»: mixes by style, named by their artists («A, B и
    /// другие»), with a cover collage. Empty while too little is listened.
    pub style_mixes: Vec<MixCard>,
    /// «Новое для тебя»: up to 30 songs of the services new to the user
    /// (they play from the stream; «Слушать всё» is [`MixId::Discover`]).
    pub discover: Vec<StreamTrackRow>,
    /// «Новые релизы»: albums of the last 60 days by artists you listen to.
    pub new_releases: Vec<StreamAlbumCard>,
    /// Albums played lately, most recent first: library albums and streamed
    /// ones.
    pub continue_listening: Vec<ContinueCard>,
    /// «Миксы исполнителей»: a click plays the artist's radio
    /// ([`MixId::Artist`]).
    pub artist_mixes: Vec<ArtistCard>,
    pub long_unplayed: Vec<ReleaseCard>,
    pub recently_added: Vec<ReleaseCard>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReleaseSort {
    #[default]
    Artist,
    Title,
    Year,
    Added,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrackSort {
    #[default]
    Artist,
    Title,
    Album,
    Added,
    Duration,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryView {
    pub playlists: Vec<PlaylistCard>,
    /// The albums liked as a whole («Любимые альбомы»), shown on a shelf
    /// of their own or among the playlists: each with the index in
    /// [`Self::playlists`] it goes before (its length: after them all), so
    /// both read latest first.
    pub liked_albums: Vec<(usize, ReleaseCard)>,
    pub releases: Vec<ReleaseCard>,
    pub artists: Vec<ArtistCard>,
    pub release_sort: ReleaseSort,
    pub track_count: usize,
    /// A window of the virtualized track list: rows `tracks_offset..`.
    pub tracks_offset: usize,
    pub tracks: Vec<TrackRow>,
    pub track_sort: TrackSort,
    pub loaded: bool,
    /// Deleted tracks ("Корзина"), most recent first; filled by [`Command::LoadTrash`].
    pub trash: Vec<TrashItem>,
    /// «Скачано»: tracks with a file by the day it came, the latest first;
    /// filled by [`Command::LoadDownloaded`].
    pub downloaded: Vec<DownloadedDay>,
    /// The «Всё» overview's rows: the latest albums in, and the most played.
    pub recently_added: Vec<ReleaseCard>,
    pub often_played: Vec<ReleaseCard>,
    /// The library filter on «Треки» ([`Command::FilterTracks`]): the query
    /// it answers and its tracks, best first.
    pub track_hits: Option<(String, Vec<TrackRow>)>,
}

/// The unified search ([`Command::Search`]): the library, Deezer, YouTube
/// Music, Spotify, torrents. The library part is filled at once, the
/// services' sections as they answer; on the «Всё» page
/// ([`SearchSources::mixed`]) also one merged answer ([`Self::answer`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchView {
    pub query: String,
    /// Library hits (and stream hits that turned out to be library tracks).
    pub tracks: Vec<TrackRow>,
    pub releases: Vec<ReleaseCard>,
    pub artists: Vec<ArtistCard>,
    pub playlists: Vec<PlaylistCard>,
    pub searching: bool,
    /// Which sources are asked (the chips).
    pub sources: SearchSources,
    /// One section per registered, enabled and chosen service, Spotify first.
    pub streams: Vec<StreamSearchSection>,
    /// The «Всё» page: the library and the services as one answer.
    pub answer: SearchAnswer,
    /// Torrent search (Prowlarr) runs only on request (or with the torrents
    /// chip on): it is slow.
    pub torrents: TorrentSearch,
}

/// Results of [`Command::SearchTorrents`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TorrentSearch {
    pub query: String,
    pub searching: bool,
    /// "Prowlarr не настроен", network errors…
    pub error: Option<String>,
    pub results: Vec<TorrentResult>,
}

/// One release found by the indexer.
#[derive(Clone, Debug, PartialEq)]
pub struct TorrentResult {
    /// Index for [`Command::DownloadTorrent`] / [`Command::ListTorrentFiles`].
    pub index: usize,
    pub title: String,
    /// "FLAC", "FLAC 24/96", "MP3 320"… guessed from the title.
    pub format: Option<String>,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub indexer: String,
    /// Best pick for automatic download (format → release match → seeders).
    pub recommended: bool,
}

/// Files of a torrent, for "Выбрать файлы…".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TorrentFiles {
    /// Which search result (or `None` for a dropped .torrent / magnet,
    /// identified by `job`).
    pub result: Option<usize>,
    pub job: Option<u64>,
    pub name: String,
    pub loading: bool,
    pub error: Option<String>,
    /// (index in the torrent, path inside it, size).
    pub files: Vec<(usize, String, u64)>,
}

/// Editable tags of one track (tag editor dialog).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagEdit {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track_no: String,
    pub disc_no: String,
    pub genre: String,
    /// Filled by MusicBrainz suggestions, written along (not shown).
    pub recording_mbid: String,
    pub release_mbid: String,
}

/// State of the tag editor for one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagEditor {
    pub track: Option<TrackId>,
    /// Current values read from the file.
    pub current: TagEdit,
    pub loading: bool,
    /// MusicBrainz suggestions: (label, values).
    pub suggestions: Vec<(String, TagEdit)>,
    pub looking_up: bool,
    pub error: Option<String>,
}

/// One torrent being seeded ("Раздача" tab).
#[derive(Clone, Debug, PartialEq)]
pub struct SeedRow {
    pub id: u64,
    pub title: String,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    /// When it will be cleaned up ("через 12 дн.").
    pub expires: Option<String>,
    /// Upload and download right now, bytes per second.
    pub up_bps: u64,
    pub down_bps: u64,
    /// Peers connected right now.
    pub peers: u32,
    /// What it takes on disk (the selected files).
    pub size_bytes: u64,
}

/// An import or download job shown in "Загрузки".
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    pub title: String,
    pub subtitle: String,
    /// 0..1, None = indeterminate.
    pub progress: Option<f32>,
    pub rate: Option<String>,
    pub eta: Option<String>,
    pub paused: bool,
    /// It can be cancelled (a torrent, an import: what it added goes back).
    pub controllable: bool,
    /// It can be paused too (a torrent).
    pub pausable: bool,
    pub error: Option<String>,
    /// When it ended (ms since the epoch); "История" is newest first by it.
    /// 0 while it runs.
    pub at: i64,
}

/// An item waiting for the user to pick a match ("Подтверди").
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub id: u64,
    pub title: String,
    pub origin: String,
    /// What the options answer ("Это какой трек?", "Что из этого скачать?").
    pub question: String,
    pub options: Vec<ConfirmOption>,
    /// What `ConfirmChoice { option: None }` does ("Пропустить",
    /// "Оставить теги файла").
    pub skip_label: String,
    /// Button for the chosen option ("Скачать", "Импортировать").
    pub accept_label: String,
    /// A card of songs each ticked on its own (the fake FLACs offered as
    /// MP3s): `options` are its songs, none is ticked at first, and both
    /// buttons act on the ticked ones only ([`Command::ConfirmPicked`]).
    /// `None`: one option is chosen ([`Command::ConfirmChoice`]).
    pub pick: Option<ConfirmPick>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmOption {
    pub label: String,
    pub detail: String,
    pub recommended: bool,
}

/// The songs of a [`Confirm::pick`] card and the words of its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmPick {
    /// The source of each option, in order: what
    /// [`Command::ConfirmPicked`] carries for the ticked ones.
    pub sources: Vec<nikgob_core::model::SourceId>,
    /// The accept button around the count of ticked songs: («Превратить»,
    /// «в MP3») reads «Превратить 3 в MP3».
    pub accept: (String, String),
    /// The other button, the same way: («Оставить», «FLAC») reads
    /// «Оставить 3 FLAC».
    pub skip: (String, String),
}

impl ConfirmPick {
    /// A button's words around `n` ticked songs; with none ticked, without
    /// a count («Превратить в MP3»).
    pub fn words(around: &(String, String), n: usize) -> String {
        let (before, after) = around;
        match (n, after.is_empty()) {
            (0, true) => before.clone(),
            (0, false) => format!("{before} {after}"),
            (n, true) => format!("{before} {n}"),
            (n, false) => format!("{before} {n} {after}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DownloadsView {
    pub active: Vec<Job>,
    pub confirm: Vec<Confirm>,
    pub history: Vec<Job>,
    pub seeding: Vec<SeedRow>,
    /// Total uploaded / limit for the "Раздача" header.
    pub seeded_bytes: u64,
    /// Files of a torrent the user is choosing from.
    pub files: Option<TorrentFiles>,
    /// Liked stream tracks waiting for (or on their way to) a file.
    pub wishlist: WishlistView,
    /// The download sources, in the user's order (Settings → «Источники»).
    pub sources: Vec<SourceView>,
    /// `[soulseek]` of settings.toml as «Источники» shows it.
    pub soulseek: SoulseekView,
    /// What «Раздача» says about both kinds of sharing.
    pub sharing: SharingView,
}

/// «Загрузки → Раздача»: how much the network is used and how much was
/// given away, torrents and Soulseek apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharingView {
    /// Right now, bytes per second.
    pub torrent_up_bps: u64,
    pub torrent_down_bps: u64,
    pub slsk_up_bps: u64,
    pub slsk_down_bps: u64,
    /// Given away today (the local day) and over all time.
    pub torrent_today: u64,
    pub slsk_today: u64,
    pub torrent_total: u64,
    pub slsk_total: u64,
    /// What the torrents being seeded take on disk.
    pub seeding_disk_bytes: u64,
    /// Peers connected to all torrents now.
    pub torrent_peers: u32,
    pub slsk: SlskSharingView,
}

impl SharingView {
    pub fn up_bps(&self) -> u64 {
        self.torrent_up_bps + self.slsk_up_bps
    }

    pub fn down_bps(&self) -> u64 {
        self.torrent_down_bps + self.slsk_down_bps
    }

    pub fn today(&self) -> u64 {
        self.torrent_today + self.slsk_today
    }

    pub fn total(&self) -> u64 {
        self.torrent_total + self.slsk_total
    }
}

/// The Soulseek half of «Раздача».
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlskSharingView {
    /// An account (or a service) is saved.
    pub configured: bool,
    /// The client (or the link to the service) runs.
    pub running: bool,
    /// On a server or a small device (`nikgob-slsk`).
    pub remote: bool,
    /// Sharing is on in settings.toml.
    pub share: bool,
    /// What is shared, as the client counts it (0 before the first scan).
    pub files: u32,
    pub folders: u32,
    /// KiB/s for all uploads together, 0 = no limit.
    pub upload_limit_kbps: u32,
    /// Uploads at once.
    pub upload_slots: u32,
    /// Running, queued, then the last finished.
    pub uploads: Vec<UploadRow>,
}

/// One file someone takes from us over Soulseek.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadRow {
    pub id: u64,
    pub user: String,
    /// The file name, without folders.
    pub file: String,
    pub size: u64,
    pub sent: u64,
    pub bytes_per_sec: u64,
    pub state: UploadRowState,
    /// Ms since the epoch when it ended (0 while not ended).
    pub ended_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadRowState {
    Queued,
    Sending,
    Done,
    Failed,
}

/// A transient message ("Добавлено в Любимые · Отменить").
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub action: Option<(String, Command)>,
    /// A place the toast's button takes the user to (used when there is no
    /// `action`): «Открыть» → «Хочу скачать», «Войти» → the service's row.
    pub open: Option<(String, Link)>,
    pub at: Instant,
    /// What the message is about, for its icon and accent.
    pub kind: ToastKind,
    /// It stays twice as long (8 s): it says how to undo a step that
    /// matters («Карточка скрыта…» with «Вернуть»).
    pub long: bool,
    /// How many times the same message came again while it was up: each
    /// starts its time over.
    pub again: u32,
}

/// A place in the app a message can lead to; the design system navigates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Route(Route),
    /// A row of the settings (each design system knows where it is).
    Setting(SettingLink),
}

/// Settings rows other parts of the app point to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingLink {
    /// The Spotify account (login).
    Spotify,
    /// «Звук из Spotify» (the playback login).
    SpotifyPlayback,
    /// The YouTube Music account.
    Youtube,
    /// Prowlarr and the torrents.
    Torrents,
    /// «Что качать по лайку».
    LikeDownload,
}

/// What a toast reports: drawn with its own icon and accent colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastKind {
    #[default]
    Info,
    /// Something got done ("Сохранено", "Добавлено в очередь").
    Success,
    /// Something started and goes on in the background ("Добавляю раздачу…").
    Working,
    /// Something failed or is not available.
    Error,
}

impl ToastKind {
    /// Guesses the kind from the (Russian) message text, so call sites only
    /// say what happened.
    pub fn guess(text: &str) -> Self {
        let t = text.to_lowercase();
        let any = |words: &[&str]| words.iter().any(|w| t.contains(w));
        if any(&[
            "ошибка", "не получилось", "не удалось", "недоступ", "не читается", "нельзя", "не запустил",
            "некуда", "нечего", "нет файла", "нет локального", "это не ", "устарел", "не скачать", "не всё",
            "не нашёл", "не нашел", "нужно войти", "не подключ", "нет связи",
        ]) {
            Self::Error
        } else if t.ends_with('…') || any(&["начался", "качается", "добавляю"]) {
            Self::Working
        } else if any(&["сохранен", "сохранён", "добавлено", "скачано", "восстановлено", "удалён", "удалено", "следующим", "в любимых", "убран", "в корзине", "объединено", "скопирован", "заменено", "вошёл", "вход выполнен", "подключён", "включён", "в «хочу скачать»", "в библиотеке"]) {
            Self::Success
        } else {
            Self::Info
        }
    }
}

/// The revision of the lyrics defaults: 1 turned word timing off for the
/// playing song and for the library, recognition from scratch off, and
/// line timing for plain texts on; 2 made «Окно» the look of the lyrics
/// control; 3 turned word timing for the playing song on and moved the
/// lead from [`OLD_LYRICS_LEAD`] to [`LYRICS_LEAD`]; 4 made «По слогам»
/// the unit of the lyrics' animations ([`Prefs::lyrics_unit`]).
pub const LYRICS_REV: u8 = 4;

/// [`Prefs::lyrics_unit`]: letter by letter («По буквам») …
pub const LYRICS_BY_LETTERS: u8 = 0;
/// … syllable by syllable («По слогам», the default) …
pub const LYRICS_BY_SYLLABLES: u8 = 1;
/// … or whole written words («По словам»).
pub const LYRICS_BY_WORDS: u8 = 2;

/// Seconds the lyrics highlight runs ahead of the voice by default
/// ([`Prefs::lyrics_lead`]): about what karaoke apps use, which makes up
/// for the frame being shown a little after it is drawn and for a word's
/// light easing in.
pub const LYRICS_LEAD: f32 = 0.2;
/// The default lead before [`LYRICS_REV`] 3.
pub const OLD_LYRICS_LEAD: f32 = 0.1;

/// User-facing settings the UI edits (the runtime persists them).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub design: Design,
    pub theme: ThemeMode,
    pub contrast: u8,
    /// Index into the design system's scheme variants (MD3E: Content first,
    /// TonalSpot «Спокойная» by default).
    pub scheme_variant: u8,
    pub ambient: Ambient,
    pub fps_limit: u32,
    pub reduced_motion: bool,
    /// Hints over buttons on hover (the settings' preview cards show anyway).
    pub tooltips: bool,
    pub gapless: bool,
    pub crossfade_s: f32,
    pub normalization: bool,
    /// «Перемешивание остаётся включённым»: a new album, playlist, mix,
    /// radio or search result keeps shuffle as it was (off: it turns
    /// shuffle off, «Перемешать» turns it on).
    pub keep_shuffle: bool,
    /// «Перемешивать радио и автовоспроизведение»: with shuffle on, the
    /// picks of a radio, autoplay and the daylist play shuffled too (off:
    /// they keep their own order).
    pub shuffle_radio: bool,
    pub opus_bitrate: u32,
    pub check_fake_lossless: bool,
    /// A "lossless" file plainly made from a lossy one is offered in
    /// «Подтверди» as an MP3 that keeps all its content really has (tags and
    /// all; the old file goes to the Trash, «Вернуть FLAC» brings it back).
    /// Off: it stays as it is, labelled for what it is, and is not offered.
    pub convert_fake_lossless: bool,
    /// …and becomes the MP3 without asking. Off unless the user says.
    pub fake_flac_auto: bool,
    pub library_dir: PathBuf,
    pub inbox_dir: PathBuf,
    /// Where lossless masters (`.flac` originals and `.json` sidecars) are kept.
    /// Defaults to `<library_dir>/.musiclib/masters`, or another drive/folder chosen by the user.
    pub masters_dir: PathBuf,
    pub fallback_seed: u32,
    pub prowlarr_url: String,
    pub prowlarr_configured: bool,
    pub seed_limit_gb: u32,
    pub seed_days: u32,
    pub no_seed: bool,
    pub output_devices: Vec<String>,
    pub output_device: Option<String>,
    /// MusicBrainz contact (User-Agent); the AcoustID key lives in settings.toml.
    pub metadata_contact: String,
    pub acoustid_configured: bool,
    /// Stop seeding a torrent at this upload/download ratio (0 = no limit).
    pub seed_ratio: f32,
    /// KiB/s, 0 = unlimited.
    pub download_limit_kbps: u32,
    pub upload_limit_kbps: u32,
    /// Content beside the cover in Now Playing.
    pub np_slot: NpSlot,
    /// The dotted ring around the Now Playing cover turns while playing.
    pub spin_ring: bool,
    /// How synced lyrics animate: 0 karaoke, 1 focus, 2 wave. Prefs saved
    /// with 3 (the old «По словам» animation) move to the wave by words
    /// ([`Prefs::upgrade_lyrics`]).
    pub lyrics_style: u8,
    /// A text with line times only (no word times) uses the calm Focus
    /// under every animation and unit instead of guessing the words' pace.
    pub wave_syllables_only: bool,
    /// How high wave letters (or words) jump as they are sung (0: they only light up).
    pub lyrics_wave_jump: f32,
    /// «Скорость прыжка»: how fast a wave letter (or word) jumps and
    /// settles (0.5..2, 1: Material's expressive fast spatial spring).
    pub lyrics_jump_speed: f32,
    /// «Скорость подсветки»: how fast a letter or word lights up, 0
    /// smoothly .. 1 at once (karaoke by letters: how soft the front is).
    pub lyrics_light_speed: f32,
    /// «Плавность»: 0 lights and moves exactly letter by letter .. 1 the
    /// karaoke front glides and the wave's jumps spread over neighbouring
    /// letters (motions averaged over up to 0.4 s; the times still lead).
    pub lyrics_smooth: f32,
    /// How far lines other than the current one fade (0 none .. 1 strong).
    pub lyrics_dim: f32,
    /// Text size multiplier for synced lyrics (0.8..1.4).
    pub lyrics_scale: f32,
    /// Seconds the lyrics highlight runs ahead of the voice (negative:
    /// behind it), in every animation; seeking and stored times keep theirs.
    pub lyrics_lead: f32,
    /// Lines further from the current one follow the scroll more lazily.
    pub lyrics_cascade: bool,
    /// The sung part glows a little.
    pub lyrics_glow: bool,
    /// Seconds after a manual scroll before the view returns to the song.
    pub lyrics_return_s: f32,
    /// The pause indicator between lines: 0 flat thin, 1 wavy thin,
    /// 2 flat thick, 3 wavy thick (the four Material 3 Expressive forms).
    pub lyrics_gap_style: u8,
    /// Crests around the pause ring (wavelength = circumference / this).
    pub lyrics_gap_waves: f32,
    /// «Высота волн» of the pause ring (0.5..2, 1 the Material height).
    pub lyrics_gap_height: f32,
    /// «Размер индикатора»: the pause ring's size (0.8..1.5).
    pub lyrics_gap_size: f32,
    /// The last seconds of a pause count down in digits instead.
    pub lyrics_countdown: bool,
    /// The old «Подсвечивать» switch (whole written words instead of
    /// letters), before [`Self::lyrics_unit`]: read once by
    /// [`Prefs::upgrade_lyrics`].
    pub lyrics_by_words: bool,
    /// «Подсвечивать»: what every animation goes over, [`LYRICS_BY_LETTERS`],
    /// [`LYRICS_BY_SYLLABLES`] (the timed syllables of a text that has
    /// them, the timed words of one that has words) or [`LYRICS_BY_WORDS`]
    /// (each part of a hyphenated word).
    pub lyrics_unit: u8,
    /// «Мягкие края»: lines near the top and bottom of the lyrics blur and
    /// fade out instead of being cut.
    pub lyrics_soft_edges: bool,
    /// «Сила размытия»: how much a line at the very edge is blurred
    /// (0..1.5, a multiplier).
    pub lyrics_soft_strength: f32,
    /// «Глубина»: how far into the lyrics each soft edge reaches, as a
    /// share of their height (0.08..0.4).
    pub lyrics_soft_depth: f32,
    /// «Затухание»: how early a line thins out in the soft edge (0: only
    /// right at the edge .. 1: from where the band starts).
    pub lyrics_soft_fade: f32,
    /// Embed the selected online lyrics in the track tags.
    pub save_found_lyrics: bool,
    /// «Видеокарта»: the lyrics models run on the GPU (DirectML), else on
    /// the CPU (also when the GPU fails).
    pub lyrics_gpu: bool,
    /// «Проверять текст по записи»: a fetched text is aligned to the
    /// track's own audio before it is trusted.
    pub lyrics_check: bool,
    /// «Подсветка по словам во всех песнях»: the background pass makes
    /// word timing for every song with a file (hours on a big library).
    pub lyrics_words_all: bool,
    /// «Подсветка по строкам для текстов без времени»: the playing song's
    /// plain text gets line timing from the recording when no source has
    /// line timing.
    pub lyrics_lines_auto: bool,
    /// «Подсветка по словам для песен, которые слушаю»: the playing song's
    /// texts are shown with word timing made from the recording.
    pub lyrics_words_playing: bool,
    /// «Вид»: what the control under the lyrics opens: 0 its menu
    /// («Меню»), 1 a window over the player («Окно», the default), 2 the
    /// «Текст песни» page («Экран»).
    pub lyrics_ui: u8,
    /// Which defaults of the lyrics settings the prefs were written with
    /// ([`LYRICS_REV`]); older prefs are moved to the new defaults once.
    pub lyrics_rev: u8,
    /// «Распознавать текст, если его нет нигде» (experimental).
    pub lyrics_recognize: bool,
    /// «Отделять голос от музыки» before aligning.
    pub lyrics_separate: bool,
    /// «Глубокое создание субтитров»: timing made with the stronger
    /// separator and aligners (docs/lyrics-align.md, «Deep mode»), slower.
    pub lyrics_deep: bool,
    /// «Скачивать модели сами» when they are needed.
    pub lyrics_models_auto: bool,
    /// «Искать на Genius».
    pub lyrics_genius: bool,
    /// «Подсветка для песен без файла»: timing may be made for a stream
    /// (the audio fetched for a while, then deleted).
    pub lyrics_streams: bool,
    /// What a stream's made timing keeps in its artist's folder: 0 the
    /// text alone («Только текст»), 1 the song as a file and the text
    /// («Песню и текст»).
    pub lyrics_stream_keep: u8,
    /// «Настройки текста…» was picked from the control's menu once: the
    /// menu stops offering it.
    pub lyrics_settings_seen: bool,
    /// The outline the Now Playing cover takes while playing.
    pub cover_shape: CoverShape,
    /// The cover's outline turns while playing (the picture stays put).
    pub cover_spin: bool,
    /// The cover swells a little on each beat.
    pub cover_pulse: bool,
    /// The dotted ring around the cover is shown.
    pub ring: bool,
    /// The ring's dots jump with the music.
    pub ring_beat: bool,
    /// What the ring is made of and where it sits.
    pub ring_style: RingStyle,
    /// The ring turns counter-clockwise.
    pub ring_ccw: bool,
    /// The cover's outline turns counter-clockwise.
    pub cover_ccw: bool,
    /// Now Playing shows a volume slider (otherwise just the mute button;
    /// the wheel still sets the volume).
    pub np_volume: bool,
    /// Instead of a fixed slider: a volume pill pops up at the bottom
    /// centre of Now Playing only while the volume changes.
    pub np_volume_pop: bool,
    /// How long the volume pill stays after the last change, in seconds.
    pub np_volume_pop_hold: f32,
    /// How Now Playing opens and closes: 0 smooth, 1 lively, 2 fast.
    pub np_motion: u8,
    /// Progress bars turn into a wave while playing (off: always flat).
    pub progress_wave: bool,
    /// The shapes [`CoverShape::Random`] picks from; empty = all of them.
    pub random_shapes: Vec<CoverShape>,
    /// Windows starts the app at sign-in (the app reads the system's own
    /// state at start; this only mirrors and edits it).
    pub autostart: bool,
    /// A start at sign-in stays in the tray, with no window.
    pub autostart_hidden: bool,
    /// «Быстрое открытие из трея»: closing the window only hides it, so it
    /// comes back at once (off: it is destroyed and made again, which saves
    /// memory while it waits in the tray).
    pub tray_keep_window: bool,
    /// A click on the Now Playing cover toggles play / pause.
    pub cover_click_pause: bool,
    /// A double click in Now Playing changes the cover outline: 0 never,
    /// 1 only in the random mode, 2 always (a fixed shape steps on).
    pub dbl_click_shape: u8,
    /// How long a tap on the cover waits for a second one, in seconds;
    /// 0 pauses at once (a second tap then undoes it).
    pub np_tap_wait: f32,
    /// Seconds a snackbar with a timeout stays up (one with an undo twice
    /// as long); 0 = until it is closed.
    pub toast_s: u8,
    /// Material shapes behind Now Playing that follow the music.
    pub bg_shapes: BgShapes,
    /// How many of them (4..=32).
    pub bg_shapes_count: u32,
    /// Size multiplier (0.5..=2).
    pub bg_shapes_size: f32,
    /// Opacity multiplier (0.25..=2).
    pub bg_shapes_opacity: f32,
    /// Motion speed multiplier (0.25..=3).
    pub bg_shapes_speed: f32,
    /// How strongly they react to the music (0..=2).
    pub bg_shapes_react: f32,
    /// Colours: 0 theme containers, 1 accent only, 2 bright accents,
    /// 3 the user's hue ([`Self::bg_shapes_hue`]).
    pub bg_shapes_colors: u8,
    /// The hue for colour mode 3: an index into the UI's swatches (0..8).
    pub bg_shapes_hue: u8,
    /// How strong the ambient clouds are (0.25..=2).
    pub ambient_strength: f32,
    /// The navigation rail always stays narrow (icons only) and its expand
    /// button is gone.
    pub rail_compact: bool,
    /// The parts of the side pane by the content, each on its own: none by
    /// default, and a part that is off has no button, menu item or key.
    pub pane_queue: bool,
    pub pane_lyrics: bool,
    pub pane_info: bool,
    /// Deleting moves tracks into the Trash for 30 days; off: they are
    /// deleted for good at once (after a confirmation).
    pub use_trash: bool,
    /// Release groups whose editions the user chose to keep apart.
    pub kept_editions: Vec<String>,

    // ---- Sources (stage 3, the "Источники" settings tab). No secrets
    // here: the Spotify Client ID and YouTube cookies live in settings.toml
    // ([`Command::SetStreamSecrets`]).
    /// Spotify is used (search, likes, sound when the account has it).
    pub spotify_enabled: bool,
    /// YouTube Music is used.
    pub youtube_enabled: bool,
    /// The Deezer catalogue is used (search, albums, discographies; no
    /// account, its tracks play from YouTube Music).
    pub deezer_enabled: bool,
    /// SoundCloud is used (search, pages and sound work without an account).
    pub soundcloud_enabled: bool,
    /// What a like of a stream track downloads: the track (from its
    /// album's torrent) or the whole album. An album like is always the album.
    pub like_download_scope: WishlistScope,
    /// Likes download at all («Ничего» turns it off: a like is only a like;
    /// «Скачать» still downloads).
    pub like_downloads: bool,
    /// Sync mode of a playlist newly linked to a service.
    pub playlist_sync_default: PlaylistSyncMode,
    /// Likes and unlikes here go to the services the track is in.
    pub push_likes: bool,
    /// A like of an album likes all its tracks too (into «Любимые» and the
    /// services). Off: it only saves the album.
    pub album_like_tracks: bool,
    /// The saved albums sit among the playlists. Off: a shelf of their own,
    /// «Любимые альбомы».
    pub liked_albums_with_playlists: bool,
    /// A like of a single's song saves the single as an album too.
    pub single_like_saves: bool,
    /// An import of export files reads the songs saved in the music library
    /// (Takeout's «music library songs»): they teach the taste only, no
    /// likes, no downloads, not in the library. Off: that file is skipped.
    pub import_saved_songs: bool,
    /// The library lists only tracks with a file (the inconspicuous switch).
    pub library_only_downloaded: bool,
    /// How many wishlist items search / download at once on their own.
    pub auto_download_limit: u32,
    /// The download sources in the order the wishlist prefers them at one
    /// quality, each on or off («Источники скачивания», docs/stage4.5.md).
    /// A source missing here takes its default place, on.
    pub download_sources: Vec<SourceSwitch>,
    /// Toasts about downloads going on by themselves: a release not found, a
    /// torrent started or finished. Off: that is only in «Загрузки» and «Хочу
    /// скачать».
    pub download_toasts: bool,
    /// «Искать качество лучше»: how often the library's files below
    /// [`Self::upgrade_below`] are searched for a better copy.
    pub upgrade_every: UpgradeEvery,
    /// What is good enough: a file below it is searched for again.
    pub upgrade_below: UpgradeBar,
    /// A better copy found replaces the file by itself. Off: it waits in
    /// «Подтверди» (a release can differ from the one the user has).
    pub upgrade_auto: bool,
    /// Size limit of the stream audio cache per service, in MB (0 = off):
    /// recently played YouTube files and Spotify's encrypted files.
    pub stream_cache_mb: u32,
    /// settings.toml has a personal Spotify Client ID (the id is not here).
    pub spotify_personal_app: bool,
    /// settings.toml has YouTube Music cookies.
    pub youtube_cookies_set: bool,
    /// «Найти песню…»: the last scope chosen, «И в YouTube Music» (true) or
    /// «Только здесь».
    pub replace_in_youtube: bool,

    // ---- Recommendations (stage 4, docs/stage4.md).
    /// «Новое в радио»: how much music new to the user radios and mixes mix
    /// in.
    pub radio_new_share: RadioNewShare,
    /// «Миксы только из скачанного»: mixes, moods, tiles, the daylist and
    /// radios pick only tracks with a file (no streams, no discoveries).
    pub mixes_downloaded_only: bool,
    /// «Миксы для тебя» → «С новым»: about every fifth song of a mix is new
    /// (from the services); «Только моё» (false) plays only the library.
    pub mixes_with_new: bool,
    /// «Карточка наверху главной» («Миры»): off, Главная starts with the
    /// greeting.
    pub home_card: bool,
    /// «Как к тебе обращаться»: the name in the greeting of Главная («Добрый
    /// вечер, Ник»); empty, the greeting alone ([`user_name`]).
    pub user_name: String,
    /// Главная asks for the name once: set on the first start (no prefs
    /// file before), gone with «Готово» or «Пропустить».
    pub name_prompt: bool,
    /// The worlds taken off the card's row («Убрать из ряда»), by key.
    pub home_hidden_worlds: Vec<String>,
    /// «Анализ звука стримов»: liked and playlist tracks without a file are
    /// analysed from YouTube Music in the background.
    pub stream_analysis: bool,
    /// «Автовоспроизведение»: when the queue's last track plays, it goes on
    /// with similar tracks by itself (a radio of that track, from where
    /// [`Self::mixes_downloaded_only`] and [`Self::radio_new_share`] say).
    pub autoplay: bool,
    /// «Продолжать мир после альбома»: after an album, autoplay plays the
    /// next tracks of its world (a soundtrack franchise, an artist's
    /// discography) in album order first, then the similar ones.
    pub autoplay_continue_world: bool,
    /// The share of new music by the kind of radio (a track's, an
    /// artist's, a mix, autoplay): what was chosen for it last; a kind not
    /// here takes [`Self::radio_new_share`].
    pub radio_new_share_by: std::collections::BTreeMap<RadioKind, RadioNewShare>,

    // ---- Dislikes (docs/TODO.md §10).
    /// «Кнопка «Не нравится»» next to ♡ in Now Playing.
    pub dislike_button: bool,
    /// «Пропускать по дизлайку»: a dislike of the playing track skips it.
    pub dislike_skips: bool,
    /// «Пропускать и в своём»: disliked tracks and artists not recommended
    /// are skipped in the user's own albums and playlists too (off: only in
    /// mixes, radios and autoplay).
    pub dislike_skip_everywhere: bool,
    /// «Учитывать в рекомендациях»: a dislike lowers its nearest sound
    /// neighbours a little too (off: only the track itself is left out).
    pub dislike_in_recs: bool,

    // ---- Taste profiles (profiles-v1).
    /// «Фон меньше влияет на рекомендации»: a play of a background session
    /// counts a quarter in the taste profiles' balance (off: fully).
    pub background_counts_less: bool,
    /// «Делить профили по звуку»: a kind of song is two profiles when its
    /// sounds part (off: one profile a kind).
    pub profiles_split: bool,
    /// «Названия профилей»: by the kind of song, or by the artists.
    pub profile_names: ProfileNames,

    // ---- «Миксы для тебя» (phase 3).
    /// The profiles' own mixes («Русские песни»).
    pub mixes_profiles: bool,
    /// «Всё вперемешку».
    pub mixes_everything: bool,
    /// The calm and the lively side of everything loved («Спокойное»,
    /// «Погромче»).
    pub mixes_moods: bool,
    /// «На повторе».
    pub mixes_on_repeat: bool,
    /// A click on a mix's card opens its page ([`Route::Mix`]; its ▶ plays
    /// it); off: the click plays it.
    pub mix_click_opens_list: bool,
    /// «Фон» cards at most (0: none), for a user who listens in the
    /// background.
    pub background_cards: u8,
    /// «Не предлагать этот микс»: the mixes of «Миксы для тебя» left out,
    /// with the name they had (Settings → «Рекомендации» returns them).
    pub home_hidden_mixes: Vec<HiddenMix>,

    // ---- The graph of similar music (docs/recs-graph.md).
    /// «Похожее по слушателям»: what other listeners hear together (the
    /// graph kept in the library) scores the radio, the mixes and the new
    /// music; off, the graph is kept but not used.
    pub graph_use: bool,
    /// «Спрашивать YouTube Music и Deezer»: the graph asks their radios
    /// and similar artists in the background.
    pub graph_services: bool,
    /// «Спрашивать ListenBrainz и MusicBrainz»: the graph asks their open
    /// data in the background.
    pub graph_open_data: bool,
    /// «Мои плейлисты и импорт истории»: the personal graph (the order of
    /// the playlists, the sessions of an imported history).
    pub graph_personal: bool,
    /// «Без истории — по самой библиотеке»: while the history is short the
    /// library itself is the taste.
    pub library_as_taste: bool,
    /// settings.toml has a Last.fm API key (the key is not here).
    pub lastfm_configured: bool,
    /// settings.toml has a ListenBrainz token (the token is not here).
    pub listenbrainz_configured: bool,
    /// «Не повторять сыгранное»: the radio and autoplay leave out what
    /// played this many hours ago or less (and in the session) while
    /// anything else is left.
    pub radio_no_repeat_hours: u8,
    /// «Подмешивать случайное»: how much of a radio of a track or an artist
    /// (and autoplay) may be new music not known to sound like what it
    /// grows from.
    pub radio_explore: RadioExplore,
    /// «Преимущественно новые песни»: how much of a radio of a track or an
    /// artist (and autoplay) is music the user has not heard.
    pub radio_unheard: RadioUnheard,
}

/// One line of [`Prefs::download_sources`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceSwitch {
    pub key: SourceKey,
    pub on: bool,
}

impl Prefs {
    /// Every source in the user's order, the ones not listed yet in their
    /// default places (on, Soulseek off).
    pub fn source_order(&self) -> Vec<SourceSwitch> {
        let mut out: Vec<SourceSwitch> = Vec::new();
        for s in &self.download_sources {
            if !out.iter().any(|o| o.key == s.key) {
                out.push(*s);
            }
        }
        for (i, k) in SourceKey::ALL.iter().enumerate() {
            if out.iter().any(|o| o.key == *k) {
                continue;
            }
            // After the default sources that come before it.
            let at = SourceKey::ALL[..i]
                .iter()
                .filter_map(|prev| out.iter().position(|o| o.key == *prev))
                .max()
                .map_or(0, |p| p + 1);
            // Soulseek needs the user's own account, and Squidify's owner
            // keeps downloads closed to guests: off until switched on.
            out.insert(at, SourceSwitch { key: *k, on: !matches!(*k, SourceKey::Soulseek | SourceKey::Squidify) });
        }
        out
    }

    /// Whether `key` is switched on.
    pub fn source_on(&self, key: SourceKey) -> bool {
        self.source_order().iter().find(|s| s.key == key).is_none_or(|s| s.on)
    }

    /// Prefs written with the lyrics defaults of revision `stored` (0: from
    /// before [`Prefs::lyrics_rev`]) take the newer defaults once: those
    /// switches were never on the settings page under their new meaning,
    /// so what they hold is the old default, not a choice.
    pub fn upgrade_lyrics(&mut self, stored: u8) {
        if stored < 1 {
            // Recognition from scratch is experimental: off unless chosen.
            self.lyrics_recognize = false;
            self.lyrics_words_all = false;
            self.lyrics_words_playing = false;
            self.lyrics_lines_auto = true;
        }
        if stored < 2 && self.lyrics_ui == 0 {
            // «Окно» became the default: the old default moves, a chosen
            // window or page stays.
            self.lyrics_ui = 1;
        }
        if stored < 3 {
            // Word highlighting became the default experience: the
            // playing song's texts are shown by words the player made
            // (the switch was off by default, never a choice when off).
            self.lyrics_words_playing = true;
            // The lead moved to what karaoke apps use; one the user set
            // stays.
            if (self.lyrics_lead - OLD_LYRICS_LEAD).abs() < 1e-3 {
                self.lyrics_lead = LYRICS_LEAD;
            }
        }
        if stored < 4 {
            // «По слогам» became the default unit: «По буквам» (the old
            // default) moves to it, «По словам» chosen stays.
            self.lyrics_unit = if self.lyrics_by_words || self.lyrics_style == 3 { LYRICS_BY_WORDS } else { LYRICS_BY_SYLLABLES };
        }

        self.lyrics_rev = LYRICS_REV;
        self.upgrade_lyrics_style();
    }

    /// «По словам» was a fourth animation (`lyrics_style` 3) before it
    /// became the unit of every animation: it moves to the wave by words,
    /// once (3 is never written again).
    pub fn upgrade_lyrics_style(&mut self) {
        if self.lyrics_style == 3 {
            self.lyrics_style = 2;
            self.lyrics_by_words = true;
            self.lyrics_unit = LYRICS_BY_WORDS;
        }
    }
}

/// A mix of «Миксы для тебя» left out ([`Prefs::home_hidden_mixes`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HiddenMix {
    pub key: MixKey,
    pub title: String,
}

/// What the taste profiles are called by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProfileNames {
    /// «Японские песни», the artists below.
    #[default]
    Kind,
    /// «Eve, Radwimps и другие».
    Artists,
}

impl Prefs {
    /// The share of new music of a radio of `kind`.
    pub fn new_share_of(&self, kind: RadioKind) -> RadioNewShare {
        self.radio_new_share_by.get(&kind).copied().unwrap_or(self.radio_new_share)
    }
}

/// What a radio grows from, for the share of new music it remembers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum RadioKind {
    #[default]
    Track,
    Artist,
    Mix,
    Autoplay,
}

/// One preference changed on its own ([`Command::SetPref`]): the runtime
/// applies it to the preferences as they are when it comes, so a copy the
/// view held from before another change cannot undo that change.
#[derive(Clone, Debug, PartialEq)]
pub enum PrefPatch {
    /// «Миксы для тебя»: «С новым» (true) or «Только моё».
    MixesWithNew(bool),
    /// «Миксы только из скачанного» («Только скачанное» on Главная).
    MixesDownloadedOnly(bool),
    /// The library lists only tracks with a file.
    LibraryOnlyDownloaded(bool),
    /// «Автовоспроизведение».
    Autoplay(bool),
    /// «Продолжать мир после альбома».
    AutoplayContinueWorld(bool),
    /// Where a radio takes its tracks («Скачанное · Библиотека · С новым»):
    /// both of the fields it sets.
    RadioSource { downloaded_only: bool, new_share: RadioNewShare },
    /// A world of the card at the top of Главная taken off its row.
    HideWorld(String),
    /// The card at the top of Главная shown or hidden.
    HomeCard(bool),
    /// The outline the Now Playing cover takes.
    CoverShape(CoverShape),
    /// What stands beside the Now Playing cover.
    NpSlot(NpSlot),
    /// Windows starts the app at sign-in (as the system says).
    Autostart(bool),
    /// «Как к тебе обращаться» ([`user_name`] keeps it tidy); Главная asks
    /// no more.
    UserName(String),
    /// Главная's question of the name: `false` («Пропустить») asks no more.
    NamePrompt(bool),
    /// The share of new music of one kind of radio (the source buttons
    /// above the queue).
    RadioNewShareBy(RadioKind, RadioNewShare),
    /// The share of new music everywhere: each kind of radio takes it.
    RadioNewShare(RadioNewShare),
    DislikeButton(bool),
    DislikeSkips(bool),
    DislikeSkipEverywhere(bool),
    DislikeInRecs(bool),
    BackgroundCountsLess(bool),
    ProfilesSplit(bool),
    ProfileNames(ProfileNames),
    MixesProfiles(bool),
    MixesEverything(bool),
    MixesMoods(bool),
    MixesOnRepeat(bool),
    BackgroundCards(u8),
    /// «Не предлагать этот микс».
    HideMix(HiddenMix),
    /// A mix left out comes back.
    ShowMix(MixKey),
    GraphUse(bool),
    GraphServices(bool),
    GraphOpenData(bool),
    GraphPersonal(bool),
    LibraryAsTaste(bool),
    RadioNoRepeat(u8),
    RadioExplore(RadioExplore),
    RadioUnheard(RadioUnheard),
    /// «Настройки текста…» was picked from the lyrics control's menu.
    LyricsSettingsSeen,
}

impl PrefPatch {
    /// The change made to `p`.
    pub fn apply(&self, p: &mut Prefs) {
        match self {
            Self::MixesWithNew(on) => p.mixes_with_new = *on,
            Self::MixesDownloadedOnly(on) => p.mixes_downloaded_only = *on,
            Self::LibraryOnlyDownloaded(on) => p.library_only_downloaded = *on,
            Self::Autoplay(on) => p.autoplay = *on,
            Self::AutoplayContinueWorld(on) => p.autoplay_continue_world = *on,
            Self::RadioSource { downloaded_only, new_share } => {
                p.mixes_downloaded_only = *downloaded_only;
                p.radio_new_share = *new_share;
            }
            Self::HideWorld(key) => {
                if !p.home_hidden_worlds.contains(key) {
                    p.home_hidden_worlds.push(key.clone());
                }
            }
            Self::HomeCard(on) => p.home_card = *on,
            Self::CoverShape(s) => p.cover_shape = *s,
            Self::NpSlot(s) => p.np_slot = *s,
            Self::Autostart(on) => p.autostart = *on,
            Self::UserName(name) => {
                p.user_name = user_name(name);
                p.name_prompt = false;
            }
            Self::NamePrompt(on) => p.name_prompt = *on,
            Self::RadioNewShareBy(kind, share) => {
                p.radio_new_share_by.insert(*kind, *share);
            }
            Self::RadioNewShare(share) => {
                p.radio_new_share = *share;
                p.radio_new_share_by.clear();
            }

            Self::DislikeButton(on) => p.dislike_button = *on,
            Self::DislikeSkips(on) => p.dislike_skips = *on,
            Self::DislikeSkipEverywhere(on) => p.dislike_skip_everywhere = *on,
            Self::DislikeInRecs(on) => p.dislike_in_recs = *on,
            Self::BackgroundCountsLess(on) => p.background_counts_less = *on,
            Self::ProfilesSplit(on) => p.profiles_split = *on,
            Self::ProfileNames(n) => p.profile_names = *n,
            Self::MixesProfiles(on) => p.mixes_profiles = *on,
            Self::MixesEverything(on) => p.mixes_everything = *on,
            Self::MixesMoods(on) => p.mixes_moods = *on,
            Self::MixesOnRepeat(on) => p.mixes_on_repeat = *on,
            Self::BackgroundCards(n) => p.background_cards = (*n).min(3),
            Self::HideMix(m) => {
                if !p.home_hidden_mixes.iter().any(|h| h.key == m.key) {
                    p.home_hidden_mixes.push(m.clone());
                }
            }
            Self::ShowMix(key) => p.home_hidden_mixes.retain(|h| h.key != *key),
            Self::GraphUse(on) => p.graph_use = *on,
            Self::GraphServices(on) => p.graph_services = *on,
            Self::GraphOpenData(on) => p.graph_open_data = *on,
            Self::GraphPersonal(on) => p.graph_personal = *on,
            Self::LibraryAsTaste(on) => p.library_as_taste = *on,
            Self::LyricsSettingsSeen => p.lyrics_settings_seen = true,
            Self::RadioNoRepeat(h) => p.radio_no_repeat_hours = *h,
            Self::RadioExplore(e) => p.radio_explore = *e,
            Self::RadioUnheard(u) => p.radio_unheard = *u,
        }
    }
}

/// The longest name the greeting takes.
pub const USER_NAME_MAX: usize = 32;

/// A name as the greeting keeps it: trimmed, its spaces single, at most
/// [`USER_NAME_MAX`] characters.
pub fn user_name(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(USER_NAME_MAX).collect::<String>().trim_end().to_owned()
}

/// «Новое в радио»: the share of new music in radios and mixes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioNewShare {
    /// «Без нового».
    None,
    /// «Немного», about a fifth.
    #[default]
    Some,
    /// «Побольше», about two fifths.
    More,
}

impl RadioNewShare {
    /// The share of a queue that is new music.
    pub const fn fraction(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Some => 0.2,
            Self::More => 0.4,
        }
    }
}

/// «Подмешивать случайное»: how often a radio of a track or an artist (and
/// autoplay) plays new music not known to sound like what it grows from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioExplore {
    /// «Никогда»: only new music that sounds like it.
    Never,
    /// «Редко», about one in ten.
    #[default]
    Rare,
    /// «Иногда», about one in five (as every radio did before).
    Sometimes,
    /// «Часто», about three in ten.
    Often,
}

impl RadioExplore {
    pub const ALL: [Self; 4] = [Self::Never, Self::Rare, Self::Sometimes, Self::Often];

    /// The share of a radio at most that may be such new music (within
    /// its share of new music).
    pub const fn fraction(self) -> f32 {
        match self {
            Self::Never => 0.0,
            Self::Rare => 0.1,
            Self::Sometimes => 0.2,
            Self::Often => 0.3,
        }
    }
}

/// «Преимущественно новые песни»: how much of a radio of a track or an
/// artist (and autoplay) is music the user has not heard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioUnheard {
    /// «Как обычно».
    #[default]
    Usual,
    /// «Больше новых»: half of the library's picks at least.
    More,
    /// «Только новые»: all, while there are.
    Only,
}

impl RadioUnheard {
    pub const ALL: [Self; 3] = [Self::Usual, Self::More, Self::Only];

    /// The share of the library's picks at least that is unheard.
    pub const fn fraction(self) -> f32 {
        match self {
            Self::Usual => 0.0,
            Self::More => 0.5,
            Self::Only => 1.0,
        }
    }
}

/// Material shapes behind the open track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BgShapes {
    #[default]
    Off,
    /// Lying on the floor and jumping on the beats.
    Bounce,
    /// Drifting and glowing with their band.
    Float,
    /// Drifting and turning into other shapes on the beats.
    Morph,
}

/// The single content slot beside the Now Playing cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NpSlot {
    #[default]
    None,
    Lyrics,
    Queue,
    Info,
}

impl NpSlot {
    pub fn toggle(&mut self, slot: Self) {
        *self = if *self == slot { Self::None } else { slot };
    }
}

/// The ring around the Now Playing cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RingStyle {
    /// Dots on a soft circle.
    Circle,
    /// Dots following the cover's outline at a fixed gap.
    #[default]
    Contour,
    /// Dots around the outline that grow into bars with the spectrum
    /// (caelestia's cover visualiser).
    Bars,
}

/// The Now Playing cover's outline while playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CoverShape {
    /// No outline: the square cover as is.
    Original,
    /// A new random shape whenever a track starts.
    Random,
    #[default]
    Cookie12,
    Cookie9,
    Cookie7,
    Cookie6,
    Cookie4,
    Sunny,
    VerySunny,
    Flower,
    Clover4,
    Clover8,
    SoftBurst,
    SoftBoom,
    Puffy,
    PuffyDiamond,
    Circle,
    Pentagon,
    Gem,
    Heart,
}

impl CoverShape {
    pub const ALL: [Self; 20] = [
        Self::Original,
        Self::Random,
        Self::Cookie12,
        Self::Cookie9,
        Self::Cookie7,
        Self::Cookie6,
        Self::Cookie4,
        Self::Sunny,
        Self::VerySunny,
        Self::Flower,
        Self::Clover4,
        Self::Clover8,
        Self::SoftBurst,
        Self::SoftBoom,
        Self::Puffy,
        Self::PuffyDiamond,
        Self::Circle,
        Self::Pentagon,
        Self::Gem,
        Self::Heart,
    ];

    /// The shapes [`CoverShape::Random`] picks from.
    pub fn shapes() -> &'static [Self] {
        &Self::ALL[2..]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Оригинал (квадрат)",
            Self::Random => "Случайная",
            Self::Cookie12 => "Печенье, 12 волн",
            Self::Cookie9 => "Печенье, 9 волн",
            Self::Cookie7 => "Печенье, 7 волн",
            Self::Cookie6 => "Печенье, 6 волн",
            Self::Cookie4 => "Печенье, 4 волны",
            Self::Sunny => "Солнце",
            Self::VerySunny => "Яркое солнце",
            Self::Flower => "Цветок",
            Self::Clover4 => "Клевер, 4 листа",
            Self::Clover8 => "Клевер, 8 листьев",
            Self::SoftBurst => "Вспышка",
            Self::SoftBoom => "Взрыв",
            Self::Puffy => "Облако",
            Self::PuffyDiamond => "Пухлый ромб",
            Self::Circle => "Круг",
            Self::Pentagon => "Пятиугольник",
            Self::Gem => "Самоцвет",
            Self::Heart => "Сердце",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Design {
    #[default]
    Md3e,
    Nothing,
    Glass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ambient {
    Off,
    Glow,
    #[default]
    Reactive,
}

/// Everything a design system needs to draw a frame.
/// Releases that are editions of one album (one MusicBrainz release
/// group), offered to fold into one; nothing merges without a yes.
#[derive(Clone, Debug, PartialEq)]
pub struct EditionOffer {
    pub id: u64,
    pub artist: String,
    /// The fullest edition: the others would join it.
    pub keep: String,
    pub others: Vec<String>,
    /// Songs both have (they would become one track each) and songs only
    /// the others have (they would join the album).
    pub same_songs: u32,
    pub new_songs: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub player: PlayerView,
    pub queue: QueueView,
    pub lyrics: LyricsView,
    pub align: AlignView,
    pub info: TrackInfo,
    pub home: HomeView,
    pub library: LibraryView,
    pub release: Option<ReleaseDetail>,
    pub artist: Option<ArtistDetail>,
    pub playlist: Option<PlaylistDetail>,
    pub search: SearchView,
    pub downloads: DownloadsView,
    /// Albums that look like editions of one album, offered to merge.
    pub editions: Vec<EditionOffer>,
    pub tag_editor: TagEditor,
    pub toasts: Vec<Toast>,
    pub prefs: Prefs,
    /// The streaming services, Spotify first (registered or not).
    pub accounts: Vec<AccountView>,
    /// The open album / playlist / artist of a service ([`Route::Stream`]).
    pub stream_page: Option<StreamPageView>,
    /// The open world of Главная ([`Route::World`]).
    pub world: Option<WorldView>,
    /// The last [`Command::Share`].
    pub share: Option<ShareView>,
    /// What the last check of Prowlarr found ([`Command::CheckIndexer`],
    /// also after its settings are saved).
    pub indexer_check: Option<IndexerCheck>,
    /// Bytes of stream audio cached on disk, all services together.
    pub stream_cache_bytes: u64,
    /// Text the app should put on the clipboard (one-shot, see
    /// [`ClipboardRequest`]).
    pub clipboard: Option<ClipboardRequest>,
    /// The open «Найти песню…» dialog ([`Command::FindSong`]).
    pub find_song: Option<FindSongView>,
    /// How far the background sound analysis is.
    pub analysis: AnalysisProgress,
    /// Disliked tracks and artists not to recommend («Не нравится»,
    /// «Не рекомендовать исполнителя»).
    pub dislikes: DislikesView,
    /// Bumped when cover files on disk may have been rewritten (an import,
    /// a retag), so a UI looks at the covers it holds again.
    pub cover_files: u64,
    /// Real album art found for the video thumbnails of stream tracks drawn
    /// on screen ([`Command::RealCovers`]): the thumbnail's url → the art's.
    pub real_covers: std::sync::Arc<std::collections::HashMap<String, String>>,
    /// Bumped on every change, so a UI can skip work when nothing changed.
    pub revision: u64,
}

/// What the user does not want: tracks disliked and artists not to
/// recommend, newest first, with what the list in Settings shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DislikesView {
    pub tracks: Vec<DislikedTrack>,
    pub artists: Vec<(ArtistId, String)>,
}

impl DislikesView {
    pub fn track(&self, id: TrackId) -> bool {
        self.tracks.iter().any(|t| t.id == id)
    }

    pub fn artist(&self, id: ArtistId) -> bool {
        self.artists.iter().any(|a| a.0 == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DislikedTrack {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
}

/// How much of the library has its sound analysed (the quiet «Анализирую
/// звук библиотеки · 40%» lines): files, and stream-only tracks that are
/// liked or in a playlist, separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnalysisProgress {
    pub files_done: u32,
    pub files_total: u32,
    pub streams_done: u32,
    pub streams_total: u32,
}

impl AnalysisProgress {
    /// Everything analysed (also true for an empty library).
    pub fn complete(&self) -> bool {
        self.files_done >= self.files_total && self.streams_done >= self.streams_total
    }

    /// Done share of files and streams together, 0..=1 (1 when there is
    /// nothing to analyse).
    pub fn fraction(&self) -> f32 {
        let total = self.files_total + self.streams_total;
        if total == 0 {
            1.0
        } else {
            ((self.files_done + self.streams_done) as f32 / total as f32).min(1.0)
        }
    }
}

impl ViewState {
    pub fn account(&self, service: Service) -> Option<&AccountView> {
        self.accounts.iter().find(|a| a.service == service)
    }
}

/// The answer of Prowlarr to a check, in words: «отвечает · 12 трекеров»,
/// «ключ не подходит».
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerCheck {
    pub checking: bool,
    pub ok: bool,
    pub text: String,
}

/// Where the user is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    Library(LibraryTab),
    Downloads(DownloadsTab),
    Settings,
    /// «Текст песни»: every text of the playing song and its timing.
    Lyrics,
    Release(ReleaseId),
    Artist(ArtistId),
    Playlist(PlaylistId),
    /// An album, playlist or artist of a service (load it with
    /// [`Command::LoadStream`]; shown from [`ViewState::stream_page`]).
    Stream(StreamPage),
    /// A world of Главная's card by its key ([`Command::LoadWorld`]).
    World(String),
    /// A mix's page: one of «Миксы для тебя» ([`HomeView::lanes`]) or,
    /// with `world`, of that world's page ([`WorldView::mixes`]).
    Mix { key: MixKey, world: Option<String> },
    /// «Новые релизы» of Главная, all of them ([`HomeView::new_releases`]).
    NewReleases,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LibraryTab {
    #[default]
    All,
    Playlists,
    Albums,
    Artists,
    Tracks,
    /// Tracks by the day their file came (experimental).
    Downloaded,
    /// Deleted tracks.
    Trash,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DownloadsTab {
    #[default]
    Active,
    Confirm,
    Seeding,
    History,
    /// Liked stream tracks waiting for a file ([`DownloadsView::wishlist`]).
    Wishlist,
}

/// What the UI asks the runtime to do. Loads are requests too: the runtime
/// answers by filling the matching part of [`ViewState`].
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    // Playback
    TogglePlay,
    Play,
    Pause,
    Next,
    /// Restart the track if past 3 s, else go back.
    Previous,
    Seek(Duration),
    SeekBy(i64),
    /// Show this track's lyrics from another source; `None` = automatic.
    PickLyricsSource(TrackId, Option<LyricsSource>),
    /// Look this track's lyrics up online again, whatever was found or not
    /// found before (a text found earlier stays until a new one comes).
    SearchLyricsAgain(TrackId),
    /// Make line or word timing for a source's text from the recording
    /// ([`AlignView::craft`]); `deep`: with deep mode's parts for this one
    /// timing, whatever `Prefs::lyrics_deep` says (which decides only the
    /// timing the player makes by itself).
    CraftLyrics { track: TrackId, source: LyricsSource, target: LyricsSync, deep: bool },
    /// Forget a failed [`AlignView::craft`] (its message was seen).
    ClearCraft,
    /// Stop the timing being made ([`AlignView::craft`]); the text stays
    /// as it was.
    CancelCraft,
    /// See this track's lyrics by words, by lines or without timing
    /// ([`LyricsView::shows`]); remembered per track.
    ShowLyricsAs(TrackId, LyricsSync),
    /// Keep the shown text (with the timing the player made) on the
    /// computer ([`LyricsView::savable`]).
    SaveLyrics { track: TrackId, source: LyricsSource },
    /// Keep the text the user edited by hand as their own: `lines` with
    /// their timing (`timed`), or untimed, their texts one a line. A `.ttml`
    /// next to the file (for a song without one in its artist's folder),
    /// marked as theirs ([`LyricsSource::Own`]): shown first from then on,
    /// never refetched.
    SaveOwnLyrics { track: TrackId, lines: Vec<LyricLine>, timed: bool },
    /// «Вернуть найденный»: the user's own text goes to the Trash (with
    /// undo) and the found texts are shown again.
    DropOwnLyrics(TrackId),
    /// The undo of [`Command::DropOwnLyrics`].
    RestoreOwnLyrics(TrackId),
    /// Download (true) or delete a lyrics model by its [`AlignModel::key`].
    AlignModel { key: String, download: bool },
    SetVolume(f32),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    PlayRelease { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    PlayPlaylist { id: PlaylistId, start: Option<TrackId>, shuffle: bool },
    PlayArtist { id: ArtistId, shuffle: bool },
    /// These tracks from the one at `start` (the user picked it).
    PlayTracks { tracks: Vec<TrackId>, start: usize, context: String },
    /// A whole list with no track picked («Слушать всё»): from the first,
    /// or from a random one under shuffle.
    PlayAll { tracks: Vec<TrackId>, context: String },
    PlayNext(Vec<TrackId>),
    Enqueue(Vec<TrackId>),
    /// All tracks of an album / an artist to the end of the queue.
    /// The whole album right after the current track, in its order (under
    /// «Только скачанное» as playing it); the queue goes on after it.
    PlayReleaseNext(ReleaseId),
    EnqueueRelease(ReleaseId),
    EnqueueArtist(ArtistId),
    PlayQueueEntry(u64),
    RemoveFromQueue(u64),
    /// Puts queue `entry` right after (or before) the `anchor` entry.
    MoveInQueue { entry: u64, anchor: u64, after: bool },
    ClearQueue,
    SaveQueueAsPlaylist(String),
    /// A new playlist named so with these tracks («Сохранить как плейлист»
    /// of a mix).
    SaveTracksAsPlaylist { name: String, tracks: Vec<TrackId> },

    // Library
    Like(LikeTarget, bool),
    CreatePlaylist(String),
    RenamePlaylist(PlaylistId, String),
    DeletePlaylist(PlaylistId),
    AddToPlaylist(PlaylistId, Vec<TrackId>),
    RemoveFromPlaylist(PlaylistId, TrackId),
    MoveInPlaylist { playlist: PlaylistId, track: TrackId, to: usize },
    /// Delete a track: into the Trash (30 days), or for good when
    /// [`Prefs::use_trash`] is off.
    TrashTrack(TrackId),
    /// Take a deleted track back out of the Trash.
    UndoTrash(TrackId),
    /// Delete every track of an album (like [`Command::TrashTrack`]).
    TrashRelease(ReleaseId),
    /// Take deleted tracks back out of the Trash.
    RestoreTracks(Vec<TrackId>),
    /// Delete tracks for good: files, master and rows.
    DeleteForever(Vec<TrackId>),
    /// Delete every track of an album for good.
    DeleteReleaseForever(ReleaseId),
    /// Delete everything in the Trash for good.
    EmptyTrash,
    /// Fill [`LibraryView::trash`].
    LoadTrash,
    /// Fill [`LibraryView::downloaded`].
    LoadDownloaded,
    /// Collage / smart cover (a custom one is set by [`Command::SetPlaylistCover`]).
    SetPlaylistCoverMode(PlaylistId, PlaylistCoverMode),

    // Loads
    LoadLibrary,
    LoadTracks { offset: usize, limit: usize, sort: TrackSort },
    /// The library filter on «Треки»: library tracks matching the words
    /// into [`LibraryView::track_hits`]; an empty query clears it.
    FilterTracks(String),
    SortReleases(ReleaseSort),
    LoadRelease(ReleaseId),
    LoadArtist(ArtistId),
    /// Fills [`ViewState::world`] with the world of this key.
    LoadWorld(String),
    LoadPlaylist(PlaylistId),
    LoadHome,
    Search(String),

    // Tags
    /// Read the file's tags into [`ViewState::tag_editor`].
    OpenTagEditor(TrackId),
    /// Ask MusicBrainz for matches of the current editor values.
    LookupTags { track: TrackId, query: TagEdit },
    /// Write tags to the file and the library.
    SaveTags { track: TrackId, tags: TagEdit },
    CloseTagEditor,
    SetPlaylistCover(PlaylistId, PathBuf),

    // Torrents
    /// Search the indexer (Prowlarr) for releases.
    SearchTorrents(String),
    /// Download a search result; `files` = indices inside the torrent
    /// (None = everything).
    DownloadTorrent { result: usize, files: Option<Vec<usize>> },
    /// Fill [`DownloadsView::files`] for a search result.
    ListTorrentFiles(usize),
    CloseTorrentFiles,
    /// Remove finished torrents from seeding now (their files go too).
    CleanSeeding,

    // Import & downloads
    ImportFiles(Vec<PathBuf>),
    ImportFolder(PathBuf),
    AddTorrentFile(PathBuf),
    AddMagnet(String),
    PauseJob(u64, bool),
    CancelJob(u64),
    /// Every fake FLAC turned into an MP3 comes back from the Trash and plays
    /// again; the MP3s go to the Trash.
    RestoreConvertedFlac,
    ConfirmChoice { id: u64, option: Option<usize> },
    /// The ticked songs of a [`Confirm::pick`] card: accepted (`accept`,
    /// the fakes become MP3s) or not (kept as they are, never offered
    /// again). Only these sources change, never the rest of the card.
    ConfirmPicked { id: u64, sources: Vec<nikgob_core::model::SourceId>, accept: bool },
    /// «В файле другая песня» on a local track: its file goes to the
    /// Trash, the track is looked for again, and that file and the place it
    /// came from are never taken for it again.
    WrongFile(nikgob_core::model::TrackId),
    /// Takes [`Self::WrongFile`] back: the file plays again.
    UndoWrongFile(nikgob_core::model::TrackId),
    /// Folds the editions of [`EditionOffer::id`] into one album.
    MergeEditions(u64),
    /// Keeps them apart and never offers them again.
    KeepEditionsApart(u64),

    // Settings
    /// Every preference at once (the Settings page edits its copy).
    SetPrefs(Box<Prefs>),
    /// One preference, applied to the preferences as they are then.
    SetPref(PrefPatch),
    /// Move lossless FLAC/WAV masters to another drive or folder (copy -> hash verify -> delete old).
    MoveMastersDir(PathBuf),
    /// Prowlarr address and API key (stored in settings.toml, never shown again).
    SetIndexer { url: String, api_key: String },
    /// The Soulseek account and where it runs (stored in `[soulseek]` of
    /// settings.toml); an empty password or token keeps the saved one.
    SetSoulseek(SoulseekForm),
    /// Soulseek's sharing, applied at once: share the library, and the
    /// upload limit in KiB/s (0 = none).
    SetSoulseekShare { share: bool, upload_limit_kbps: u32 },
    /// Soulseek uploads at once (`upload_slots` in settings.toml).
    SetSoulseekSlots(u32),
    /// Asks Prowlarr whether it answers ([`ViewState::indexer_check`]) at the
    /// address and key typed in the settings (not saved yet); an empty address
    /// checks the saved one, an empty key uses the saved key.
    CheckIndexer { url: String, api_key: String },
    /// MusicBrainz contact and AcoustID key (stored in settings.toml).
    SetMetadata { contact: String, acoustid_key: Option<String> },
    /// The Last.fm API key of the graph of similar music → settings.toml
    /// (never into prefs); empty clears it.
    SetLastfmKey(String),
    /// The ListenBrainz token of the graph's popularity → settings.toml
    /// (never into prefs); empty clears it.
    SetListenbrainzToken(String),
    /// Video thumbnails (`i.ytimg.com`) of stream tracks drawn on screen:
    /// their real album art is looked up ([`ViewState::real_covers`]).
    RealCovers(Vec<String>),
    DismissToast(u64),

    // Streaming: accounts (stage 3). Every network call runs in the
    // background; the result shows up in `ViewState::accounts` / a toast.
    /// Logs in ([`AccountView::busy`] meanwhile; the prompt to act on is in
    /// [`AccountView::account`]). A Spotify personal app's Client ID is
    /// saved to settings.toml on success.
    StreamLogin { service: Service, input: LoginInput },
    StreamLogout(Service),
    /// A step of Spotify Soloist's setup ([`AccountView::soloist`]).
    Soloist(SoloistAction),
    /// Secrets → settings.toml (never into prefs): `None` keeps the value,
    /// `Some("")` clears it.
    SetStreamSecrets {
        spotify_client_id: Option<String>,
        youtube_cookies: Option<String>,
    },

    // Streaming: search and pages
    /// The chips of the unified search; runs the current query again.
    SetSearchSources(SearchSources),
    /// Fill [`ViewState::stream_page`].
    LoadStream(StreamPage),
    /// Play stream tracks (rows of a search, page or section). They become
    /// "seen" tracks (a library row outside the library) as they play.
    PlayStreamTracks {
        tracks: Vec<RemoteId>,
        start: usize,
        context: String,
    },
    /// Play the merged tracks of the «Всё» search: library tracks and
    /// stream tracks in one queue ([`SearchAnswer::tracks`]).
    PlaySearch {
        tracks: Vec<SearchPick>,
        start: usize,
        context: String,
    },
    /// Like [`Command::PlayRelease`] for an album of a service.
    PlayStreamAlbum {
        id: RemoteId,
        start: Option<RemoteId>,
        shuffle: bool,
    },
    /// Like [`Command::PlayPlaylist`] for a playlist of a service.
    PlayStreamPlaylist {
        id: RemoteId,
        start: Option<RemoteId>,
        shuffle: bool,
    },
    /// Stream tracks after the current one / at the end of the queue.
    PlayStreamNext(Vec<RemoteId>),
    EnqueueStream(Vec<RemoteId>),
    /// «Не нравится» (`dislike`) or its undo for a track: out of every
    /// recommendation, skipped now when it plays ([`Prefs::dislike_skips`]),
    /// its like gone. A track liked or played to the end often is asked
    /// about first, unless `sure`; `relike` gives the like back with an
    /// undo.
    Dislike { track: TrackId, dislike: bool, sure: bool, relike: bool },
    /// Like a stream track: it joins the library (marked "стрим"), the like

    /// goes to the services ([`Prefs::push_likes`]) and, without a file, a
    /// wishlist item downloads it ([`Prefs::like_download_scope`]).
    LikeStream { track: RemoteId, on: bool },
    /// Like every track of an album of a service (the wishlist downloads
    /// the whole album).
    LikeStreamAlbum { album: RemoteId, on: bool },
    /// Download stream tracks without liking them: each goes to «Хочу
    /// скачать» and joins the library once its file is here (not as a like).
    DownloadStream { tracks: Vec<RemoteId> },
    /// The same for a whole album of a service (one album item).
    DownloadStreamAlbum(RemoteId),
    /// Library tracks without a file → «Хочу скачать» (no like).
    DownloadTracks(Vec<TrackId>),
    /// The missing files of a library album → one album item of «Хочу
    /// скачать».
    DownloadRelease(ReleaseId),
    /// Stream tracks into a library playlist (they join the library).
    AddStreamToPlaylist(PlaylistId, Vec<RemoteId>),
    /// A song.link / album.link for a track or album → [`ViewState::share`]
    /// and the clipboard, plus a toast.
    Share(ShareTarget),

    // Streaming: wishlist
    /// Fill [`DownloadsView::wishlist`].
    LoadWishlist,
    /// Start downloading these waiting items (queued under
    /// [`Prefs::auto_download_limit`]).
    WishlistDownload(WishlistSelect),
    /// Never offer these again (also not by a pull or a re-import).
    WishlistDismiss(Vec<WishlistId>),
    /// Take dismissed items back: they wait again (imported ones) or are
    /// searched for at once.
    WishlistRestore(Vec<WishlistId>),
    /// Search a torrent for this item now (instead of next week).
    WishlistRetry(WishlistId),
    /// Search again now for every item nothing was found for (a new source
    /// may have it).
    WishlistRetryAll,
    /// Save the audio of the item's track from YouTube Music as it is (low
    /// quality, a torrent upgrades it later).
    WishlistSaveFromYoutube(WishlistId),
    /// The user's own .torrent / magnet / file / folder for this item: it is
    /// downloaded / imported as the item's track.
    WishlistAttach { item: WishlistId, what: WishlistAttach },

    // Streaming: cold start, sync, discography. Not done yet: answered with
    // a "Скоро" toast (the UI can be built against them).
    /// Likes, own playlists and recent plays from a service's API; progress
    /// as a job in "Загрузки"; likes and playlists → library (stream) +
    /// wishlist "ждёт", history → events only.
    ImportFromService(Service),
    /// Export files (Spotify Extended Streaming History / Account data,
    /// Google Takeout YouTube Music): a folder or a .zip.
    ImportExportFiles(PathBuf),
    /// Empties the stream audio cache of every service.
    ClearStreamCache,
    /// The app window was shown, hidden (tray, minimized) or (un)focused.
    /// While it is hidden and nothing plays, the runtime's periodic network
    /// work (like pulls, playlist checks, completing imports) waits; on
    /// focus a like pull older than 10 minutes runs.
    WindowState { visible: bool, focused: bool },
    /// «Загрузки» is (not) the page shown. Off it, the runtime publishes a
    /// torrent's changes but not its bytes, rates and progress, so seeding
    /// does not wake the window every second.
    DownloadsShown(bool),
    /// How a library playlist linked to a service follows it.
    SetPlaylistSync { playlist: PlaylistId, mode: PlaylistSyncMode },
    /// A playlist of a service into the library: a library playlist that
    /// follows it ([`Prefs::playlist_sync_default`]; someone else's
    /// playlist is only followed).
    FollowStreamPlaylist(RemoteId),
    /// Fill [`ArtistDetail::discography`] ("Нет в библиотеке").
    LoadDiscography(ArtistId),

    // Streaming: a song by hand for a clip or a wrong match
    /// Opens [`ViewState::find_song`] for this stream track and searches
    /// songs for `query` (`None`: its cleaned title and artist).
    FindSong { track: TrackId, query: Option<String> },
    CloseFindSong,
    /// The track becomes the song `with` (a result of the dialog); with
    /// `youtube` the like moves there too: the liked clip is unliked on
    /// YouTube Music and the song liked. The choice is kept as
    /// [`Prefs::replace_in_youtube`].
    ReplaceTrack { track: TrackId, with: RemoteId, youtube: bool },
    /// Takes a replacement back (the toast's «Отменить»), on YouTube Music
    /// too when it went there.
    UndoReplace(u64),

    // Recommendations (stage 4, docs/stage4.md §6)
    /// Plays a mix card: «Твой микс» as it is today, a mood, a style mix,
    /// an artist's radio or the discoveries; everything but «Твой микс» is
    /// endless (the queue becomes a radio, [`QueueView::radio`]).
    PlayMix { id: MixId },
    /// «Радио по треку» / «Радио» of an artist: the seed first (a track),
    /// then a queue that extends itself and learns from skips.
    StartRadio { seed: RadioSeed },
    /// «Не рекомендовать трек / исполнителя» (`dislike`), or its undo (the
    /// toast's «Отменить» sends it with `dislike: false`). Kept out of
    /// every mix, radio and discovery from now on.
    RecFeedback { target: RecTarget, dislike: bool },
    /// «Обновить» in ⋮ of a mix: «Твой микс» is picked again (and kept for
    /// the rest of the day), «Новое для тебя» is looked for again, a mix of
    /// «Миксы для тебя» gets «Другой состав».
    RefreshMix { id: MixId },
    /// «Смешать с…»: a blend of these taste profiles (two or three, the
    /// card's first) plays and stands first in «Миксы для тебя» until
    /// 06:00.
    PlayBlend { profiles: Vec<u32> },
    /// «Скрыть карточку»: switches «Карточка наверху главной» off and says
    /// how to bring it back (the snackbar's «Вернуть»).
    HideHomeCard,
    /// «Скачать всё» of a mix whose songs have no file yet («Ещё не
    /// скачано»): every stream song of it goes to «Хочу скачать».
    DownloadMix { id: MixId },
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            design: Design::Md3e,
            theme: ThemeMode::Dark,
            contrast: 0,
            scheme_variant: 1,
            ambient: Ambient::Reactive,
            fps_limit: 60,
            reduced_motion: false,
            tooltips: false,
            gapless: true,
            crossfade_s: 0.0,
            normalization: true,
            keep_shuffle: true,
            shuffle_radio: false,
            opus_bitrate: 160,
            check_fake_lossless: true,
            convert_fake_lossless: true,
            fake_flac_auto: false,
            library_dir: PathBuf::new(),
            inbox_dir: PathBuf::new(),
            masters_dir: PathBuf::new(),
            fallback_seed: 0xFF6750A4,
            prowlarr_url: String::new(),
            prowlarr_configured: false,
            seed_limit_gb: 50,
            seed_days: 30,
            no_seed: false,
            output_devices: Vec::new(),
            output_device: None,
            metadata_contact: String::new(),
            acoustid_configured: false,
            seed_ratio: 2.0,
            download_limit_kbps: 0,
            upload_limit_kbps: 0,
            np_slot: NpSlot::None,
            spin_ring: true,
            lyrics_style: 0,
            wave_syllables_only: true,
            lyrics_wave_jump: 1.0,
            lyrics_jump_speed: 1.0,
            lyrics_light_speed: 0.65,
            lyrics_smooth: 0.5,
            lyrics_dim: 0.5,
            lyrics_scale: 1.0,
            lyrics_lead: LYRICS_LEAD,
            lyrics_cascade: true,
            lyrics_glow: false,
            lyrics_return_s: 4.0,
            lyrics_gap_style: 1,
            lyrics_gap_waves: 6.0,
            lyrics_gap_height: 1.0,
            lyrics_gap_size: 1.0,
            lyrics_countdown: true,
            lyrics_by_words: false,
            lyrics_unit: LYRICS_BY_SYLLABLES,
            lyrics_soft_edges: true,
            lyrics_soft_strength: 1.0,
            lyrics_soft_depth: 0.18,
            lyrics_soft_fade: 0.4,
            save_found_lyrics: false,
            // DirectML is Windows' own: the CPU is the only choice elsewhere.
            lyrics_gpu: cfg!(windows),
            lyrics_check: true,
            lyrics_words_all: false,
            lyrics_lines_auto: true,
            lyrics_words_playing: true,
            lyrics_rev: LYRICS_REV,
            lyrics_ui: 1,
            lyrics_recognize: false,
            lyrics_separate: true,
            lyrics_deep: false,
            lyrics_models_auto: true,
            lyrics_genius: true,
            lyrics_streams: true,
            lyrics_stream_keep: 0,
            lyrics_settings_seen: false,
            cover_shape: CoverShape::default(),
            cover_spin: true,
            cover_pulse: true,
            ring: true,
            ring_beat: true,
            ring_style: RingStyle::default(),
            ring_ccw: false,
            cover_ccw: true,
            np_volume: false,
            np_volume_pop: false,
            np_volume_pop_hold: 0.8,
            np_motion: 1,
            progress_wave: true,
            random_shapes: Vec::new(),
            autostart: false,
            autostart_hidden: true,
            tray_keep_window: true,
            cover_click_pause: true,
            dbl_click_shape: 1,
            np_tap_wait: 0.22,
            toast_s: 4,
            bg_shapes: BgShapes::Off,
            bg_shapes_count: 14,
            bg_shapes_size: 1.0,
            bg_shapes_opacity: 1.0,
            bg_shapes_speed: 1.0,
            bg_shapes_react: 1.0,
            bg_shapes_colors: 0,
            bg_shapes_hue: 5,
            ambient_strength: 1.0,
            rail_compact: false,
            pane_queue: false,
            pane_lyrics: false,
            pane_info: false,
            use_trash: true,
            kept_editions: Vec::new(),
            spotify_enabled: true,
            youtube_enabled: true,
            deezer_enabled: true,
            soundcloud_enabled: true,
            like_download_scope: WishlistScope::Track,
            like_downloads: true,
            playlist_sync_default: PlaylistSyncMode::Follow,
            push_likes: true,
            album_like_tracks: false,
            liked_albums_with_playlists: false,
            single_like_saves: false,
            import_saved_songs: true,
            library_only_downloaded: false,
            auto_download_limit: 2,
            download_sources: Vec::new(),
            download_toasts: false,
            upgrade_every: UpgradeEvery::Week,
            upgrade_below: UpgradeBar::Lossless,
            upgrade_auto: false,
            stream_cache_mb: 500,
            spotify_personal_app: false,
            youtube_cookies_set: false,
            replace_in_youtube: false,
            radio_new_share: RadioNewShare::Some,
            mixes_downloaded_only: false,
            mixes_with_new: false,
            home_card: true,
            user_name: String::new(),
            name_prompt: false,
            home_hidden_worlds: Vec::new(),
            home_hidden_mixes: Vec::new(),
            stream_analysis: true,
            autoplay: true,
            autoplay_continue_world: true,
            radio_new_share_by: std::collections::BTreeMap::new(),
            dislike_button: true,
            dislike_skips: true,
            dislike_skip_everywhere: false,
            dislike_in_recs: true,
            background_counts_less: true,
            profiles_split: true,
            profile_names: ProfileNames::Kind,
            mixes_profiles: true,
            mixes_everything: true,
            mixes_moods: true,
            mixes_on_repeat: true,
            mix_click_opens_list: true,
            background_cards: 3,
            graph_use: true,
            graph_services: true,
            graph_open_data: true,
            graph_personal: true,
            library_as_taste: true,
            lastfm_configured: false,
            listenbrainz_configured: false,
            radio_no_repeat_hours: 3,
            radio_explore: RadioExplore::Rare,
            radio_unheard: RadioUnheard::Usual,
        }
    }
}

impl Prefs {
    /// The prefs of a first start (no prefs file yet): the player as its
    /// owner tuned it, with nothing of theirs (folders, accounts, name),
    /// not starting with Windows, and Главная asks for the name.
    pub fn first_start() -> Self {
        Self {
            fps_limit: 142,
            normalization: false,
            seed_ratio: 3.5,
            lyrics_wave_jump: 0.5,
            lyrics_jump_speed: 2.0,
            lyrics_light_speed: 0.9,
            lyrics_smooth: 0.85,
            lyrics_dim: 0.75,
            lyrics_scale: 0.9,
            lyrics_lead: 0.2,
            lyrics_glow: true,
            lyrics_return_s: 2.0,
            lyrics_gap_waves: 7.0,
            lyrics_gap_height: 0.5,
            lyrics_countdown: false,
            lyrics_soft_edges: false,
            lyrics_soft_strength: 0.25,
            lyrics_soft_depth: 0.29,
            save_found_lyrics: true,
            lyrics_deep: true,
            cover_shape: CoverShape::Random,
            ring_style: RingStyle::Circle,
            np_volume_pop: true,
            np_volume_pop_hold: 0.5,
            random_shapes: vec![
                CoverShape::Cookie12,
                CoverShape::Cookie9,
                CoverShape::Cookie7,
                CoverShape::Cookie6,
                CoverShape::Sunny,
                CoverShape::VerySunny,
                CoverShape::Flower,
                CoverShape::Clover4,
                CoverShape::Clover8,
                CoverShape::SoftBurst,
                CoverShape::Puffy,
                CoverShape::PuffyDiamond,
                CoverShape::Pentagon,
                CoverShape::Gem,
            ],
            autostart: false,
            autostart_hidden: false,
            bg_shapes: BgShapes::Morph,
            bg_shapes_count: 10,
            bg_shapes_opacity: 0.25,
            bg_shapes_speed: 0.65,
            bg_shapes_react: 0.5,
            bg_shapes_colors: 1,
            bg_shapes_hue: 6,
            rail_compact: true,
            spotify_enabled: false,
            library_only_downloaded: true,
            auto_download_limit: 3,
            upgrade_every: UpgradeEvery::Never,
            stream_cache_mb: 700,
            mixes_with_new: true,
            name_prompt: true,
            ..Self::default()
        }
    }
}

/// The card at the top of Главная («Миры») is in the app at all: off,
/// nobody sees it nor its row in Settings, whatever the prefs say, and
/// Главная starts with the greeting. Its code stays for its return.
pub const HOME_CARD: bool = false;

impl Prefs {
    /// The card at the top of Главная shows: the app has it
    /// ([`HOME_CARD`]) and «Карточка наверху главной» is on.
    pub const fn home_card_shown(&self) -> bool {
        HOME_CARD && self.home_card
    }

    /// The service is switched on.
    pub const fn service_enabled(&self, s: Service) -> bool {
        match s {
            Service::Spotify => self.spotify_enabled,
            Service::Youtube => self.youtube_enabled,
            Service::Deezer => self.deezer_enabled,
            Service::Soundcloud => self.soundcloud_enabled,
        }
    }
}

#[cfg(test)]
mod toast_tests {
    use super::ToastKind;

    #[test]
    fn toast_kinds_are_guessed_from_the_text() {
        for (text, kind) in [
            ("Не получилось воспроизвести трек", ToastKind::Error),
            ("Торренты недоступны", ToastKind::Error),
            ("Это не magnet-ссылка", ToastKind::Error),
            ("Восстановить удалось не всё", ToastKind::Error),
            ("Добавляю раздачу…", ToastKind::Working),
            ("Импорт начался · смотри «Загрузки»", ToastKind::Working),
            ("Сохранено", ToastKind::Success),
            ("Теги сохранены", ToastKind::Success),
            ("Плейлист «Ночь» сохранён", ToastKind::Success),
            ("Добавлено в очередь", ToastKind::Success),
            ("Альбом в Любимых · 3 трека", ToastKind::Success),
            ("Альбом в корзине (30 дней)", ToastKind::Success),
            ("Удалено навсегда: 2", ToastKind::Success),
            ("Импорт: 12 треков", ToastKind::Info),
            ("Не нашёл раздачу: Кино — Война", ToastKind::Error),
            ("Ссылка скопирована", ToastKind::Success),
            ("Spotify подключён · звук пока с YouTube Music", ToastKind::Success),
            ("Spotify не подключён", ToastKind::Error),
            ("В «Хочу скачать»: «Война»", ToastKind::Success),
            ("Нет связи — проверь интернет", ToastKind::Error),
            ("Заменено на «AIYOU»", ToastKind::Success),
            ("Скоро", ToastKind::Info),
        ] {
            assert_eq!(ToastKind::guess(text), kind, "{text}");
        }
    }
}


#[cfg(test)]
mod context_tests {
    use super::PlayerView;

    fn line(c: Option<&str>) -> String {
        PlayerView { context: c.map(str::to_owned), ..PlayerView::default() }.context_line()
    }

    #[test]
    fn contexts_read_without_quotes_in_quotes() {
        assert_eq!(line(None), "Сейчас играет");
        assert_eq!(line(Some("Библиотека")), "Играет из «Библиотека»");
        assert_eq!(line(Some("библиотеки")), "Играет из библиотеки");
        assert_eq!(line(Some("альбома «Группа крови»")), "Играет из альбома «Группа крови»");
        assert_eq!(line(Some("плейлиста «Вечер»")), "Играет из плейлиста «Вечер»");
        assert_eq!(line(Some("поиска Spotify: лёд")), "Играет из поиска Spotify: лёд");
        assert_eq!(line(Some("поиска: лёд")), "Играет из поиска: лёд");
        // Saved by older versions.
        assert_eq!(line(Some("Spotify · «лёд»")), "Играет из поиска Spotify: лёд");
        assert_eq!(line(Some("Поиск «лёд»")), "Играет из поиска: лёд");
        for c in ["Spotify · «лёд»", "Поиск «лёд»", "альбома «X»", "Кино"] {
            assert!(line(Some(c)).matches('«').count() <= 1, "{c}");
        }
    }

    #[test]
    fn the_name_of_a_page_is_the_link_of_the_header() {
        let with = |c: &str, page: Option<crate::Route>| {
            let p = PlayerView { context: Some(c.to_owned()), context_page: page, ..PlayerView::default() };
            let l = p.context_line();
            p.context_link(&l).map(|r| l[r].to_owned())
        };
        let album = Some(crate::Route::Release(crate::ReleaseId::new()));
        assert_eq!(with("альбома «Группа крови»", album.clone()).as_deref(), Some("Группа крови"));
        assert_eq!(with("Кино", Some(crate::Route::Artist(crate::ArtistId::new()))).as_deref(), Some("Кино"));
        // No page (a search, a queue of picked songs), or nothing quoted.
        assert_eq!(with("альбома «Группа крови»", None), None);
        assert_eq!(with("поиска: лёд", album.clone()), None);
        assert_eq!(with("библиотеки", album), None);
    }
}

#[cfg(test)]
mod release_day_tests {
    use super::ReleaseDay;

    #[test]
    fn dates_parse_to_the_day_or_the_month() {
        let day = |y, m, d| Some(ReleaseDay { year: y, month: m, day: d });
        assert_eq!(ReleaseDay::parse("2019-05-17", Some(2019)), day(2019, 5, Some(17)));
        assert_eq!(ReleaseDay::parse(" 2019-05-17T00:00:00Z ", None), day(2019, 5, Some(17)));
        assert_eq!(ReleaseDay::parse("2019/5/7", None), day(2019, 5, Some(7)));
        assert_eq!(ReleaseDay::parse("2019-05", Some(2019)), day(2019, 5, None));
        // A bare year says nothing the card does not.
        assert_eq!(ReleaseDay::parse("2019", Some(2019)), None);
        // A recording's date of another year is not the album's.
        assert_eq!(ReleaseDay::parse("2003-02-01", Some(2019)), None);
        for bad in ["", "19-05-17", "2019-13-01", "2019-05-32", "2019-05-17-1", "май 2019", "2019-05-00"] {
            assert_eq!(ReleaseDay::parse(bad, None), None, "{bad}");
        }
    }
}
