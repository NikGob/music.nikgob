//! Single-file rips on import: a lossless image of a whole disc (`.flac`,
//! `.wav`, `.ape`, `.wv`) with a cue sheet next to it becomes one track per
//! entry of the sheet.
//!
//! * [`plan`] looks at the folders of a job's files: which files are
//!   images of a sheet, which sheets do not fit their folder (they are
//!   logged and their files import as they are), and which images are left
//!   alone because the folder has per-track files too.
//! * [`cut`] decodes an image once and writes each track's PCM to its own
//!   WAV in the job's work folder; the pipeline then reads, identifies and
//!   encodes those like any file, with the sheet's tags
//!   ([`Segment::tags`]) and the image as their shared master.
//! * The image and its sheet are kept as they are: never changed, moved
//!   only from the Inbox (as any original is).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cue::{self, CueSheet, CueTrack};
use crate::decode::{PcmSpec, decode_file};
use crate::error::{ImportError, Result};
use crate::fsutil::{self, is_cue_file};
use crate::sanitize::{long_path, sanitize_component, truncate_chars};
use crate::spectrum::{SpectrumCollector, SpectrumReport};
use crate::tags::{self, FileInfo, TagSet};

/// Lossless containers a sheet's image may be cut from. APE and WavPack
/// decode through `ffmpeg`.
pub const IMAGE_EXTENSIONS: &[&str] = &["flac", "wav", "ape", "wv"];
/// Images only `ffmpeg` decodes.
const FFMPEG_EXTENSIONS: &[&str] = &["ape", "wv"];

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// An image file the job cuts.
#[derive(Clone, Debug)]
pub(crate) struct Image {
    pub path: PathBuf,
    pub cue: PathBuf,
    pub sheet: Arc<CueSheet>,
    /// Which `FILE` of the sheet it is.
    pub file: usize,
    pub disc_no: u32,
    pub disc_total: Option<u32>,
}

impl Image {
    pub fn tracks(&self) -> &[CueTrack] {
        &self.sheet.files[self.file].tracks
    }

    /// The image imported as it is when cutting fails (only what the
    /// pipeline can decode).
    pub fn importable_whole(&self) -> bool {
        fsutil::is_audio_file(&self.path)
    }
}

/// What [`plan`] made of a job's files.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Files imported as they are.
    pub files: Vec<PathBuf>,
    /// Images cut into tracks.
    pub images: Vec<Image>,
}

/// One folder's look: its sheets and the audio next to them.
struct Folder {
    cues: Vec<PathBuf>,
    audio: Vec<PathBuf>,
}

fn list_folder(dir: &Path) -> Folder {
    let mut f = Folder {
        cues: Vec::new(),
        audio: Vec::new(),
    };
    if let Ok(rd) = std::fs::read_dir(long_path(dir)) {
        for e in rd.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let path = dir.join(&name);
            if is_cue_file(&path) {
                f.cues.push(path);
            } else if fsutil::is_audio_file(&path) || IMAGE_EXTENSIONS.contains(&ext_of(&path).as_str()) {
                f.audio.push(path);
            }
        }
    }
    f.cues.sort();
    f.audio.sort();
    f
}

fn same_name(a: &Path, b: &str) -> bool {
    a.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == b.to_lowercase())
}

fn stem_lower(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The file a sheet's `FILE` names in its folder: the name itself (any
/// case), or the same name with another lossless extension (a sheet written
/// for `.wav` next to the `.flac` it was packed into). Nothing else is
/// guessed: a sheet naming a file that is not there does not fit.
fn resolve(sheet_path: &Path, name: &str, folder: &Folder) -> Option<PathBuf> {
    // A path from the ripper's disk (`C:\rip\CDImage.wav`): its name.
    let name = name.replace('\\', "/");
    let base = name.rsplit('/').next().unwrap_or(&name).trim();
    if base.is_empty() {
        return None;
    }
    let dir = sheet_path.parent()?;
    let exact = dir.join(&name);
    if folder.audio.contains(&exact) {
        return Some(exact);
    }
    if let Some(a) = folder.audio.iter().find(|a| same_name(a, base)) {
        return Some(a.clone());
    }
    let images: Vec<&PathBuf> = folder
        .audio
        .iter()
        .filter(|a| IMAGE_EXTENSIONS.contains(&ext_of(a).as_str()))
        .collect();
    let want = Path::new(base).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    images.into_iter().find(|a| stem_lower(a) == want).cloned()
}

/// Letters and digits only, lowercase: how a title is looked for in a
/// file name.
fn folded(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The leading number of a file name (`03 - Title`, `03.Title`).
fn leading_number(stem: &str) -> Option<u32> {
    let digits: String = stem.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether `files` (per-track files of the job, next to the image) cover
/// every track of `image`: each track is a different file whose name holds
/// the track's title (or, for a track without one, starts with its number).
fn covered_by(image: &Image, files: &[&PathBuf]) -> bool {
    let names: Vec<(String, Option<u32>)> = files
        .iter()
        .map(|f| {
            let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            (folded(&stem), leading_number(&stem))
        })
        .collect();
    let mut used = vec![false; names.len()];
    image.tracks().iter().all(|t| {
        let title = t.title.as_deref().map(folded).filter(|t| !t.is_empty());
        let hit = names.iter().enumerate().position(|(i, (name, no))| {
            !used[i]
                && match &title {
                    Some(title) => name.contains(title.as_str()),
                    None => *no == Some(t.number),
                }
        });
        if let Some(i) = hit {
            used[i] = true;
        }
        hit.is_some()
    })
}

/// Sorts a job's files into the ones imported as they are and the images
/// cut by their sheets. `cues` are sheets the job was given (a torrent's
/// files, a sheet dropped into the Inbox): their images are cut even when
/// the job does not list them. A sheet that does not fit its folder is
/// logged and changes nothing.
pub(crate) fn plan(files: Vec<PathBuf>, cues: &[PathBuf]) -> Plan {
    let mut dirs: Vec<PathBuf> = files
        .iter()
        .chain(cues)
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    dirs.dedup();
    let given_files: HashSet<&PathBuf> = files.iter().collect();
    let given_cues: HashSet<&PathBuf> = cues.iter().collect();
    let mut images: Vec<Image> = Vec::new();
    let mut leave: HashSet<PathBuf> = HashSet::new();
    for dir in &dirs {
        let folder = list_folder(dir);
        if folder.cues.is_empty() {
            continue;
        }
        // The images the sheets name (files of two tracks or more).
        let mut found: Vec<Image> = Vec::new();
        for cue_path in &folder.cues {
            let sheet = match cue::read(cue_path) {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    log::warn!("cue {}: {e}; the files next to it import as they are", cue_path.display());
                    continue;
                }
                Err(e) => {
                    log::warn!("cue {} is not read: {e}", cue_path.display());
                    continue;
                }
            };
            let resolved: Vec<Option<PathBuf>> =
                sheet.files.iter().map(|f| resolve(cue_path, &f.name, &folder)).collect();
            if let Some(i) = resolved.iter().position(Option::is_none) {
                log::warn!(
                    "cue {}: its FILE «{}» is not next to it; the files import as they are",
                    cue_path.display(),
                    sheet.files[i].name
                );
                continue;
            }
            let sheet = Arc::new(sheet);
            for (i, path) in resolved.into_iter().flatten().enumerate() {
                let image_ext = IMAGE_EXTENSIONS.contains(&ext_of(&path).as_str());
                if sheet.files[i].tracks.len() < 2 || !image_ext {
                    continue;
                }
                // Two sheets of one image (the same sheet in two encodings):
                // the first one.
                if found.iter().any(|f| f.path == path) {
                    continue;
                }
                found.push(Image {
                    path,
                    cue: cue_path.clone(),
                    sheet: sheet.clone(),
                    file: i,
                    disc_no: 1,
                    disc_total: None,
                });
            }
        }
        if found.is_empty() {
            continue;
        }
        // Per-track files of this job next to an image (some releases
        // ship both): when they are every track of it, those are taken
        // and the image is left alone; else it is cut.
        let tracks: Vec<&PathBuf> = folder
            .audio
            .iter()
            .filter(|a| given_files.contains(a) && !found.iter().any(|f| f.path == **a))
            .collect();
        for f in found {
            if covered_by(&f, &tracks) {
                log::info!(
                    "cue {}: the per-track files next to {} are its tracks; those are imported, the image is left alone",
                    f.cue.display(),
                    f.path.display()
                );
                leave.insert(f.path);
            } else if given_files.contains(&f.path) || given_cues.contains(&f.cue) {
                images.push(f);
            }
        }
    }
    number_discs(&mut images);
    let cut: HashSet<&PathBuf> = images.iter().map(|i| &i.path).collect();
    // An APE/WavPack file no sheet cuts is not read (as before).
    let files = files
        .iter()
        .filter(|f| !cut.contains(f) && !leave.contains(*f) && fsutil::is_audio_file(f))
        .cloned()
        .collect();
    Plan { files, images }
}

/// Disc numbers of the sheets of one album folder (a `CD1`/`CD2` folder
/// counts as its album's): the sheet's `REM DISCNUMBER`, else what the
/// folder or the sheet's name says, else the next free number in name
/// order; the count when the sheets are several.
fn number_discs(images: &mut [Image]) {
    let mut albums: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for i in images.iter() {
        let sheets = albums.entry(crate::folder::album_group_dir(&i.cue)).or_default();
        if !sheets.contains(&i.cue) {
            sheets.push(i.cue.clone());
        }
    }
    let mut numbers: HashMap<PathBuf, (u32, Option<u32>)> = HashMap::new();
    for sheets in albums.values_mut() {
        sheets.sort();
        let sheet_of = |cue: &PathBuf| images.iter().find(|i| i.cue == *cue).map(|i| i.sheet.clone());
        let hint = |cue: &PathBuf| -> Option<u32> {
            let sheet = sheet_of(cue)?;
            sheet.disc_no.or_else(|| {
                let dir = cue.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
                let stem = cue.file_stem().map(|n| n.to_string_lossy().into_owned());
                dir.as_deref().and_then(cue::disc_hint).or_else(|| stem.as_deref().and_then(cue::disc_hint))
            })
        };
        let many = sheets.len() > 1;
        let mut taken: HashSet<u32> = HashSet::new();
        let mut given: Vec<(PathBuf, Option<u32>)> = Vec::new();
        for cue in sheets.iter() {
            let n = hint(cue).filter(|n| !taken.contains(n));
            if let Some(n) = n {
                taken.insert(n);
            }
            given.push((cue.clone(), n));
        }
        let mut next = 1;
        for (cue, n) in given {
            let n = n.unwrap_or_else(|| {
                while taken.contains(&next) {
                    next += 1;
                }
                taken.insert(next);
                next
            });
            let total = sheet_of(&cue).and_then(|s| s.disc_total).or(many.then_some(sheets.len() as u32));
            numbers.insert(cue, (n, total.map(|t| t.max(n))));
        }
    }
    for i in images.iter_mut() {
        if let Some(&(n, total)) = numbers.get(&i.cue) {
            i.disc_no = n;
            i.disc_total = total;
        }
    }
}

// ------------------------------------------------------------------ work folder

/// How long a work folder a crashed run left behind is kept.
const STALE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// The work folder of one job under `root` (made when the first image is
/// cut). Job folders a crashed run left behind a day ago go first.
pub(crate) fn job_dir(root: &Path, job: u64, started: i64) -> PathBuf {
    if let Ok(rd) = std::fs::read_dir(long_path(root)) {
        for e in rd.flatten() {
            let old = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE);
            if old && e.file_name().to_string_lossy().starts_with("job-") && e.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_dir_all(long_path(&e.path()));
            }
        }
    }
    root.join(format!("job-{}-{job}-{started}", std::process::id()))
}

// ------------------------------------------------------------------ cutting

/// An image as cut: what every one of its tracks shares.
#[derive(Debug)]
pub(crate) struct CutImage {
    pub path: PathBuf,
    pub cue: PathBuf,
    /// BLAKE3 of the image: the master's hash.
    pub hash: String,
    pub size: u64,
    /// What the image is (its codec decides the tracks' rank).
    pub info: FileInfo,
    /// The fake-lossless check of the whole image, when it ran: every
    /// track of it is taken for what the image is.
    pub spectrum: Option<SpectrumReport>,
    pub spectrum_db: Option<Vec<f32>>,
}

/// One track cut out of an image.
#[derive(Debug)]
pub(crate) struct Segment {
    pub image: Arc<CutImage>,
    /// What the sheet says (with what the image's own tags say of the album).
    pub tags: TagSet,
}

/// The tags of one track: the sheet's, then the image's own album tags.
/// Nothing the image says of itself as a track (its title, ids) is taken.
fn segment_tags(img: &Image, track: &CueTrack, own: &TagSet) -> TagSet {
    let sheet = &img.sheet;
    let performer = track
        .performer
        .clone()
        .or_else(|| sheet.performer.clone())
        .or_else(|| own.album_artist.clone())
        .or_else(|| own.artist.clone());
    let date = sheet.date.clone().or_else(|| own.date.clone());
    let year = date
        .as_deref()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse().ok())
        .filter(|y: &i32| (1000..3000).contains(y))
        .or(own.year);
    TagSet {
        title: track.title.clone(),
        artists: performer.iter().cloned().collect(),
        artist: performer,
        album: sheet.title.clone().or_else(|| own.album.clone()),
        album_artist: sheet.performer.clone().or_else(|| own.album_artist.clone()),
        track_no: (track.number > 0).then_some(track.number),
        track_total: Some(sheet.track_count() as u32),
        disc_no: Some(img.disc_no),
        disc_total: img.disc_total,
        year,
        date,
        genre: sheet.genre.clone().or_else(|| own.genre.clone()),
        compilation: own.compilation,
        release_mbid: own.release_mbid.clone(),
        release_group_mbid: own.release_group_mbid.clone(),
        isrc: track.isrc.clone(),
        label: own.label.clone(),
        cover: own.cover.clone(),
        ..TagSet::default()
    }
}

/// A PCM WAV written as it comes (16 or 24-bit integer, else 32-bit
/// float), its sizes filled in at the end.
struct WavOut {
    w: BufWriter<File>,
    path: PathBuf,
    channels: usize,
    bits: u16,
    bytes: u64,
    buf: Vec<u8>,
}

impl WavOut {
    fn create(path: &Path, spec: PcmSpec, depth: Option<u8>) -> Result<Self> {
        let bits = match depth {
            Some(d) if d <= 16 => 16,
            Some(d) if d <= 24 => 24,
            _ => 32,
        };
        let float = bits == 32;
        let channels = u16::try_from(spec.channels).map_err(|_| ImportError::decode(path, "too many channels"))?;
        let block = channels * (bits / 8);
        let mut h = Vec::with_capacity(44);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&spec.sample_rate.to_le_bytes());
        h.extend_from_slice(&(spec.sample_rate * u32::from(block)).to_le_bytes());
        h.extend_from_slice(&block.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&0u32.to_le_bytes());
        let file = File::create(long_path(path)).map_err(fsutil::io_err(path))?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        w.write_all(&h).map_err(fsutil::io_err(path))?;
        Ok(Self {
            w,
            path: path.to_path_buf(),
            channels: spec.channels,
            bits,
            bytes: 0,
            buf: Vec::new(),
        })
    }

    /// Interleaved samples (whole frames).
    fn write(&mut self, pcm: &[f32]) -> Result<()> {
        self.buf.clear();
        match self.bits {
            16 => {
                for s in pcm {
                    let v = (f64::from(*s) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                    self.buf.extend_from_slice(&v.to_le_bytes());
                }
            }
            24 => {
                for s in pcm {
                    let v = (f64::from(*s) * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                    self.buf.extend_from_slice(&v.to_le_bytes()[..3]);
                }
            }
            _ => {
                for s in pcm {
                    self.buf.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
        self.bytes += self.buf.len() as u64;
        self.w.write_all(&self.buf).map_err(fsutil::io_err(&self.path))
    }

    fn frames(&self) -> u64 {
        self.bytes / (self.channels as u64 * u64::from(self.bits / 8)).max(1)
    }

    fn finish(mut self) -> Result<u64> {
        let data = u32::try_from(self.bytes).map_err(|_| ImportError::decode(&self.path, "track over 4 GB"))?;
        let io = fsutil::io_err(&self.path);
        let frames = self.frames();
        (|| -> std::io::Result<()> {
            self.w.flush()?;
            let f = self.w.get_mut();
            f.seek(SeekFrom::Start(4))?;
            f.write_all(&(36 + data).to_le_bytes())?;
            f.seek(SeekFrom::Start(40))?;
            f.write_all(&data.to_le_bytes())?;
            f.flush()
        })()
        .map_err(io)?;
        Ok(frames)
    }
}

/// An APE/WavPack image decoded by `ffmpeg` into a WAV in `work`.
fn ffmpeg_decode(image: &Path, work: &Path, depth: Option<u8>) -> Result<PathBuf> {
    let exe = crate::encode::FfmpegEncoder::find_exe()
        .ok_or_else(|| ImportError::Unsupported(format!("{}: ffmpeg is needed to read it", image.display())))?;
    let out = work.join("image.nikgob-decoded.wav");
    let codec = match depth {
        Some(d) if d <= 16 => "pcm_s16le",
        Some(d) if d <= 24 => "pcm_s24le",
        _ => "pcm_f32le",
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(image)
        .args(["-map", "0:a:0", "-vn", "-map_metadata", "-1", "-c:a", codec, "-f", "wav"])
        .arg(&out)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd.output().map_err(|e| ImportError::Encode(format!("cannot run ffmpeg: {e}")))?;
    if !output.status.success() {
        let _ = fsutil::remove_file(&out);
        return Err(ImportError::decode(
            image,
            format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr).trim()),
        ));
    }
    Ok(out)
}

/// The track files of one image in `work`, with what each is.
pub(crate) type Cut = Vec<(PathBuf, Arc<Segment>)>;

/// Decodes `img` once and writes each of its tracks to a WAV of its own in
/// `work` (a folder of this image alone): `NN - Title.wav`. The sheet must
/// fit the audio (no track starting past its end); on any failure what was
/// written goes and the error says why.
///
/// With `check`, the fake-lossless check runs on the whole image in the
/// same decode: one verdict for every track of it ([`CutImage::spectrum`]).
pub(crate) fn cut(img: &Image, work: &Path, check: bool) -> Result<Cut> {
    let result = cut_inner(img, work, check);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(long_path(work));
    }
    result
}

fn cut_inner(img: &Image, work: &Path, check: bool) -> Result<Cut> {
    let (own, info) = tags::read_tags(&img.path)?;
    if !info.codec.is_lossless() {
        return Err(ImportError::Unsupported(format!("{}: {} is not a lossless image", img.path.display(), info.codec)));
    }
    let tracks = img.tracks();
    // The sheet against the length the container states, before any work.
    if let Some(rate) = info.sample_rate.filter(|_| !info.duration.is_zero()) {
        let total = (info.duration.as_secs_f64() * f64::from(rate)).round() as u64;
        cue::spans(&img.sheet.files[img.file], rate, Some(total)).map_err(|e| ImportError::decode(&img.path, e.to_string()))?;
    }
    fsutil::create_dir_all(work)?;
    let hash = fsutil::hash_file(&img.path)?;
    let size = fsutil::file_size(&img.path)?;
    let decoded = if FFMPEG_EXTENSIONS.contains(&ext_of(&img.path).as_str()) {
        Some(ffmpeg_decode(&img.path, work, info.bit_depth)?)
    } else {
        None
    };
    let source = decoded.as_deref().unwrap_or(&img.path);
    let mut names: Vec<PathBuf> = Vec::with_capacity(tracks.len());
    for (i, t) in tracks.iter().enumerate() {
        let title = t.title.as_deref().map(|s| truncate_chars(&sanitize_component(s), 80));
        let mut name = match title {
            Some(title) => format!("{:02} - {title}", t.number),
            None => format!("{:02}", t.number),
        };
        // A sheet that repeats a number and title still gets a file per track.
        if names.iter().any(|n| n.file_stem().is_some_and(|s| s.to_string_lossy() == name)) {
            name = format!("{name} ({})", i + 1);
        }
        names.push(work.join(format!("{name}.wav")));
    }

    struct State {
        spans: Option<Vec<std::ops::Range<u64>>>,
        /// Frames decoded so far.
        pos: u64,
        current: usize,
        out: Option<WavOut>,
        written: Vec<u64>,
        failure: Option<ImportError>,
        spectrum: Option<SpectrumCollector>,
    }
    let mut st = State {
        spans: None,
        pos: 0,
        current: 0,
        out: None,
        written: vec![0; tracks.len()],
        failure: None,
        spectrum: None,
    };
    let duration_s = info.duration.as_secs_f64();
    let file = &img.sheet.files[img.file];
    let depth = info.bit_depth;
    let mut sink = |chunk: &[f32], spec: PcmSpec| -> ControlFlow<()> {
        let ch = spec.channels.max(1);
        if st.spans.is_none() {
            if check {
                st.spectrum = Some(SpectrumCollector::new(spec.sample_rate, spec.channels, duration_s));
            }
            match cue::spans(file, spec.sample_rate, None) {
                Ok(s) => st.spans = Some(s),
                Err(e) => {
                    st.failure = Some(ImportError::decode(&img.path, e.to_string()));
                    return ControlFlow::Break(());
                }
            }
        }
        if let Some(sp) = st.spectrum.as_mut() {
            sp.push(chunk);
        }
        let spans = st.spans.as_ref().expect("spans");
        let frames = (chunk.len() / ch) as u64;
        let mut at = 0u64;
        while at < frames {
            let pos = st.pos + at;
            // Past the current track: close it, open the next.
            while st.current < spans.len() && pos >= spans[st.current].end {
                if let Some(out) = st.out.take() {
                    match out.finish() {
                        Ok(n) => st.written[st.current] = n,
                        Err(e) => {
                            st.failure = Some(e);
                            return ControlFlow::Break(());
                        }
                    }
                }
                st.current += 1;
            }
            if st.current >= spans.len() {
                break;
            }
            let span = &spans[st.current];
            if pos < span.start {
                // Before the first track (a leading pregap): skipped.
                at += (span.start - pos).min(frames - at);
                continue;
            }
            if st.out.is_none() {
                match WavOut::create(&names[st.current], spec, depth) {
                    Ok(o) => st.out = Some(o),
                    Err(e) => {
                        st.failure = Some(e);
                        return ControlFlow::Break(());
                    }
                }
            }
            let take = (span.end - pos).min(frames - at);
            let (a, b) = ((at as usize) * ch, ((at + take) as usize) * ch);
            if let Some(out) = st.out.as_mut()
                && let Err(e) = out.write(&chunk[a..b])
            {
                st.failure = Some(e);
                return ControlFlow::Break(());
            }
            at += take;
        }
        st.pos += frames;
        ControlFlow::Continue(())
    };
    let decoded_spec = decode_file(source, &mut sink);
    if let Some(d) = &decoded {
        let _ = fsutil::remove_file(d);
    }
    let spec = decoded_spec?;
    if let Some(e) = st.failure.take() {
        return Err(e);
    }
    if let Some(out) = st.out.take() {
        st.written[st.current.min(tracks.len() - 1)] = out.finish()?;
    }
    // Every track must have sound: one that starts past the end of the
    // audio means the sheet is not this file's.
    if let Some(i) = st.written.iter().position(|&n| n == 0) {
        let rate = f64::from(spec.sample_rate.max(1));
        return Err(ImportError::decode(
            &img.path,
            cue::CueError::PastEnd {
                file: file.name.clone(),
                track: tracks[i].number,
                start_s: cue::frames_to_samples(tracks[i].start, spec.sample_rate) as f64 / rate,
                length_s: st.pos as f64 / rate,
            }
            .to_string(),
        ));
    }
    let (spectrum, spectrum_db) = match st.spectrum.take().map(SpectrumCollector::finish_with_levels) {
        Some((report, levels)) => (Some(report), levels),
        None => (None, None),
    };
    let image = Arc::new(CutImage {
        path: img.path.clone(),
        cue: img.cue.clone(),
        hash,
        size,
        info,
        spectrum,
        spectrum_db,
    });
    Ok(names
        .into_iter()
        .zip(tracks)
        .map(|(path, t)| {
            let seg = Segment {
                image: image.clone(),
                tags: segment_tags(img, t, &own),
            };
            (path, Arc::new(seg))
        })
        .collect())
}

// ------------------------------------------------------------------ analysis

/// A track file read by the pipeline is taken for what the sheet says:
/// its tags, and the rank of the image it came from (lossless, by the
/// image's codec, rate and depth). `info` keeps the track's own length.
pub(crate) fn apply(seg: &Segment, tags: &mut TagSet, info: &mut FileInfo) {
    let img = &seg.image.info;
    *tags = seg.tags.clone();
    info.codec = img.codec.clone();
    info.sample_rate = img.sample_rate.or(info.sample_rate);
    info.bit_depth = img.bit_depth.or(info.bit_depth);
    info.bitrate_kbps = img.bitrate_kbps;
}

// ------------------------------------------------------------------ masters

/// The sheet kept with an image master: `<masters>/<hash>.cue`, a copy of
/// its bytes as they came (one sheet may serve several images; an Inbox
/// sheet goes only once every image of it is kept, see [`sheet_done`]).
/// A sheet already there stays (never overwritten). Returns its place.
pub(crate) fn keep_sheet(masters_dir: &Path, sheet: &Path, hash: &str) -> Result<PathBuf> {
    let dest = masters_dir.join(format!("{hash}.cue"));
    if !long_path(&dest).exists() {
        fsutil::copy_file_verified(sheet, &dest, None)?;
    }
    Ok(dest)
}

/// What became of one image of a sheet when its job's folder was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageEnd {
    /// Kept as a master with its own copy of the sheet.
    Kept,
    /// Sent to the Trash (every track of it was a duplicate).
    Trashed,
    /// Still where it was (it failed, or the job stopped).
    Stayed,
}

/// What to do with a sheet in the Inbox once its images are done: it goes
/// when every image of it is kept with a copy of it (`Some(false)`: removed,
/// its copies are in the masters folder) or went to the Trash
/// (`Some(true)` when all did: to the Trash too); while any image stays, so
/// does the sheet (`None`).
pub(crate) fn sheet_done(ends: &[ImageEnd]) -> Option<bool> {
    if ends.is_empty() || ends.contains(&ImageEnd::Stayed) {
        None
    } else {
        Some(ends.iter().all(|e| *e == ImageEnd::Trashed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav_i16(path: &Path, frames: usize, rate: u32) {
        let spec = PcmSpec {
            sample_rate: rate,
            channels: 2,
        };
        let mut w = WavOut::create(path, spec, Some(16)).unwrap();
        // A ramp that says which frame it is: frame i is i / 32768 (mod).
        let pcm: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = ((i % 30_000) as f32) / 32768.0;
                [v, -v]
            })
            .collect();
        w.write(&pcm).unwrap();
        w.finish().unwrap();
    }

    fn sheet(text: &str) -> Arc<CueSheet> {
        Arc::new(cue::parse(text).unwrap())
    }

    #[test]
    fn a_wav_image_is_cut_sample_exact_at_index_01() {
        let dir = tempfile::tempdir().unwrap();
        let rate = 44_100;
        let wav = dir.path().join("disc.wav");
        // 3 s of audio; tracks at 0, 1.0 s (pregap from 0.8 s) and 2.5 s.
        write_wav_i16(&wav, 3 * rate as usize, rate);
        let text = "PERFORMER \"P\"\nTITLE \"Alb\"\nREM DATE 2001\nFILE \"disc.wav\" WAVE\n\
            TRACK 01 AUDIO\nTITLE \"One: a\"\nINDEX 01 00:00:00\n\
            TRACK 02 AUDIO\nTITLE \"Two\"\nPERFORMER \"Q\"\nINDEX 00 00:00:60\nINDEX 01 00:01:00\n\
            TRACK 03 AUDIO\nINDEX 01 00:02:37\n";
        let img = Image {
            path: wav.clone(),
            cue: dir.path().join("disc.cue"),
            sheet: sheet(text),
            file: 0,
            disc_no: 1,
            disc_total: None,
        };
        let work = dir.path().join("work");
        let cut = cut(&img, &work, true).unwrap();
        assert_eq!(cut.len(), 3);
        let names: Vec<String> = cut.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["01 - One - a.wav", "02 - Two.wav", "03.wav"]);
        let len = |p: &Path| {
            let r = hound::WavReader::open(p).unwrap();
            (r.duration(), r.spec().bits_per_sample)
        };
        let t3 = cue::frames_to_samples(2 * 75 + 37, rate) as u32;
        assert_eq!(len(&cut[0].0), (rate, 16), "track 1 ends at track 2's INDEX 01 (pregap included)");
        assert_eq!(len(&cut[1].0).0, t3 - rate);
        assert_eq!(len(&cut[2].0).0, 3 * rate - t3);
        // The first sample of track 2 is frame 44100 of the image, bit for bit.
        let mut r = hound::WavReader::open(&cut[1].0).unwrap();
        let first: Vec<i16> = r.samples::<i16>().take(2).map(Result::unwrap).collect();
        assert_eq!(first, vec![(44_100 % 30_000) as i16, -((44_100 % 30_000) as i16)]);
        // Tags from the sheet.
        let t = &cut[1].1.tags;
        assert_eq!(t.title.as_deref(), Some("Two"));
        assert_eq!(t.artist.as_deref(), Some("Q"));
        assert_eq!(t.album.as_deref(), Some("Alb"));
        assert_eq!(t.album_artist.as_deref(), Some("P"));
        assert_eq!((t.track_no, t.track_total, t.year), (Some(2), Some(3), Some(2001)));
        assert_eq!(cut[2].1.tags.artist.as_deref(), Some("P"), "the disc's performer");
        assert_eq!(cut[0].1.image.hash, fsutil::hash_file(&wav).unwrap());
        assert!(cut[0].1.image.info.codec.is_lossless());
    }

    #[test]
    fn a_sheet_past_the_end_of_its_file_is_refused_and_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("disc.wav");
        write_wav_i16(&wav, 44_100, 44_100);
        let img = Image {
            path: wav,
            cue: dir.path().join("disc.cue"),
            sheet: sheet("FILE \"disc.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:05:00\n"),
            file: 0,
            disc_no: 1,
            disc_total: None,
        };
        let work = dir.path().join("work");
        let err = cut(&img, &work, false).unwrap_err().to_string();
        assert!(err.contains("past the end"), "{err}");
        assert!(!work.exists());
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    const TWO: &str = "FILE \"CDImage.wav\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 01:00:00\n";

    #[test]
    fn plan_finds_images_by_their_sheets() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        // The sheet names a .wav; the image was packed into .flac.
        touch(&a.join("Artist - Album.flac"));
        std::fs::write(a.join("Artist - Album.cue"), TWO.replace("CDImage.wav", "Artist - Album.wav")).unwrap();
        touch(&a.join("cover.jpg"));
        let p = plan(vec![a.join("Artist - Album.flac")], &[]);
        assert!(p.files.is_empty());
        assert_eq!(p.images.len(), 1);
        assert_eq!(p.images[0].path, a.join("Artist - Album.flac"));
        assert_eq!((p.images[0].disc_no, p.images[0].disc_total), (1, None));

        // An APE image is found through its sheet alone.
        let b = dir.path().join("B");
        touch(&b.join("CDImage.ape"));
        std::fs::write(b.join("CDImage.cue"), TWO).unwrap();
        let p = plan(Vec::new(), &[b.join("CDImage.cue")]);
        assert_eq!(p.images.len(), 1);
        assert_eq!(p.images[0].path, b.join("CDImage.ape"));
    }

    #[test]
    fn per_track_files_next_to_an_image_win() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        touch(&a.join("CDImage.flac"));
        std::fs::write(a.join("CDImage.cue"), TWO).unwrap();
        touch(&a.join("01 - One.flac"));
        touch(&a.join("02 - Two.flac"));
        let files = vec![a.join("01 - One.flac"), a.join("02 - Two.flac"), a.join("CDImage.flac")];
        let p = plan(files, &[]);
        assert!(p.images.is_empty());
        assert_eq!(p.files, vec![a.join("01 - One.flac"), a.join("02 - Two.flac")], "the image is left out");
        // The image chosen alone is cut: the files next to it are not in the job.
        let p = plan(vec![a.join("CDImage.flac")], &[]);
        assert_eq!(p.images.len(), 1);
    }

    const TITLED: &str = "FILE \"CDImage.wav\" WAVE\n\
        TRACK 01 AUDIO\nTITLE \"Morning Song\"\nINDEX 01 00:00:00\n\
        TRACK 02 AUDIO\nTITLE \"Evening\"\nINDEX 01 01:00:00\n\
        TRACK 03 AUDIO\nTITLE \"Night\"\nINDEX 01 02:00:00\n";

    #[test]
    fn other_audio_next_to_an_image_leaves_it_out_only_when_it_is_every_track() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        touch(&a.join("CDImage.flac"));
        std::fs::write(a.join("CDImage.cue"), TITLED).unwrap();
        let image = a.join("CDImage.flac");
        // Two unrelated bonus tracks: the image is cut, the bonus tracks import.
        let bonus = vec![a.join("Bonus A.flac"), a.join("Bonus B.flac")];
        for b in &bonus {
            touch(b);
        }
        let mut files = bonus.clone();
        files.push(image.clone());
        let p = plan(files, &[]);
        assert_eq!(p.images.len(), 1, "unrelated files do not stand for the image");
        assert_eq!(p.files, bonus);
        // An incomplete set of its tracks: still cut.
        let some = vec![a.join("01 - Morning Song.flac"), a.join("02 - Evening.flac")];
        for f in &some {
            touch(f);
        }
        let mut files = some.clone();
        files.push(image.clone());
        assert_eq!(plan(files, &[]).images.len(), 1);
        // Every track by its title: the per-track files win.
        let all = a.join("03 Night.flac");
        touch(&all);
        let mut files = some.clone();
        files.push(all.clone());
        files.push(image.clone());
        let p = plan(files, &[]);
        assert!(p.images.is_empty());
        assert_eq!(p.files.len(), 3);
        assert!(!p.files.contains(&image));
    }

    #[test]
    fn a_sheet_that_does_not_fit_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        touch(&a.join("one.flac"));
        touch(&a.join("two.flac"));
        // Names a file that is not there, and the folder has two images.
        std::fs::write(a.join("x.cue"), TWO.replace("CDImage.wav", "missing.wav")).unwrap();
        let files = vec![a.join("one.flac"), a.join("two.flac")];
        let p = plan(files.clone(), &[]);
        assert!(p.images.is_empty());
        assert_eq!(p.files, files);
        // One image only, of another name: never taken for the missing file.
        let b = dir.path().join("B");
        touch(&b.join("unrelated.flac"));
        std::fs::write(b.join("other-album.cue"), TWO.replace("CDImage.wav", "missing.wav")).unwrap();
        let p = plan(vec![b.join("unrelated.flac")], &[b.join("other-album.cue")]);
        assert!(p.images.is_empty(), "a missing FILE is not guessed");
        assert_eq!(p.files, vec![b.join("unrelated.flac")]);
        // An APE no sheet names is not read at all.
        touch(&b.join("lone.ape"));
        let p = plan(vec![b.join("lone.ape")], &[]);
        assert!(p.files.is_empty() && p.images.is_empty());
        // A per-track sheet (one track per file) cuts nothing either.
        std::fs::write(
            a.join("x.cue"),
            "FILE \"one.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"two.flac\" WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        let p = plan(files.clone(), &[]);
        assert!(p.images.is_empty());
        assert_eq!(p.files, files);
    }

    #[test]
    fn an_inbox_sheet_goes_only_when_every_image_of_it_is_settled() {
        use ImageEnd::{Kept, Stayed, Trashed};
        assert_eq!(sheet_done(&[Kept, Kept]), Some(false), "copies kept with each image: removed");
        assert_eq!(sheet_done(&[Kept, Trashed]), Some(false));
        assert_eq!(sheet_done(&[Trashed, Trashed]), Some(true), "all duplicates: to the Trash");
        assert_eq!(sheet_done(&[Kept, Stayed]), None, "an image still needs it");
        assert_eq!(sheet_done(&[]), None);
    }

    #[test]
    fn a_sheet_kept_for_two_images_is_copied_for_each() {
        let dir = tempfile::tempdir().unwrap();
        let sheet = dir.path().join("Inbox").join("a.cue");
        touch(&sheet);
        let masters = dir.path().join("masters");
        std::fs::create_dir_all(&masters).unwrap();
        keep_sheet(&masters, &sheet, "h1").unwrap();
        keep_sheet(&masters, &sheet, "h2").unwrap();
        assert!(masters.join("h1.cue").is_file() && masters.join("h2.cue").is_file());
        assert!(sheet.is_file(), "the Inbox sheet is not moved by the first image");
        // A sheet kept before is never overwritten.
        std::fs::write(masters.join("h1.cue"), b"earlier").unwrap();
        keep_sheet(&masters, &sheet, "h1").unwrap();
        assert_eq!(std::fs::read(masters.join("h1.cue")).unwrap(), b"earlier");
    }

    #[test]
    fn discs_of_a_set_are_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let set = dir.path().join("Set");
        for d in ["CD1", "CD2"] {
            touch(&set.join(d).join("CDImage.flac"));
            std::fs::write(set.join(d).join("CDImage.cue"), TWO).unwrap();
        }
        let p = plan(vec![set.join("CD2").join("CDImage.flac"), set.join("CD1").join("CDImage.flac")], &[]);
        let mut discs: Vec<(u32, Option<u32>)> = p.images.iter().map(|i| (i.disc_no, i.disc_total)).collect();
        discs.sort_unstable();
        assert_eq!(discs, vec![(1, Some(2)), (2, Some(2))]);
        // Two sheets in one folder without numbers: by name.
        let one = dir.path().join("One");
        for n in ["a", "b"] {
            touch(&one.join(format!("{n}.flac")));
            std::fs::write(one.join(format!("{n}.cue")), TWO.replace("CDImage.wav", &format!("{n}.wav"))).unwrap();
        }
        let p = plan(vec![one.join("a.flac"), one.join("b.flac")], &[]);
        let d: Vec<(String, u32)> = p
            .images
            .iter()
            .map(|i| (i.path.file_name().unwrap().to_string_lossy().into_owned(), i.disc_no))
            .collect();
        assert_eq!(d, vec![("a.flac".into(), 1), ("b.flac".into(), 2)]);
    }
}
