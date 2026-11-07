//! Reading and writing the unified tag set (PLAN section 6, steps 1 and 6) with `lofty`.
//!
//! Standard fields go through lofty's generic [`Tag`]. Vorbis comments (Opus, Ogg
//! Vorbis, FLAC) are written directly with their literal keys. The one field lofty
//! cannot express generically, our own track id (`NIKGOB_TRACK_ID`), is a
//! Vorbis comment, an ID3v2 `TXXX` frame or an MP4 freeform atom, depending on the
//! container.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::Id3v2Tag;
use lofty::mp4::{Atom, AtomData, AtomIdent, Mp4Codec, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::{OggPictureStorage, OpusFile, VorbisFile, tag::VorbisComments};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{ItemKey, Tag, TagExt};
use nikgob_core::model::{Codec, TrackId};

use crate::error::{ImportError, Result};

/// Custom field holding the permanent track uuid.
pub const TRACK_ID_KEY: &str = "NIKGOB_TRACK_ID";
/// Reference loudness of EBU R128 / Opus (RFC 7845), LUFS.
pub const R128_REFERENCE_LUFS: f64 = -23.0;
/// Reference loudness of ReplayGain 2.0, LUFS.
pub const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;
const MP4_MEAN: &str = "com.nikgob";

/// The tags we read and write, in one place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagSet {
    pub title: Option<String>,
    /// Individual artists (a multi-valued ARTIST).
    pub artists: Vec<String>,
    /// Credit line as shown (`"A feat. B"`).
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_no: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<i32>,
    /// The date as stored (`2019`, `2019-05-03`).
    pub date: Option<String>,
    pub genre: Option<String>,
    pub compilation: bool,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub isrc: Option<String>,
    pub acoustid_id: Option<String>,
    /// Compressed Chromaprint (the Picard `ACOUSTID_FINGERPRINT`).
    pub acoustid_fingerprint: Option<String>,
    pub track_id: Option<TrackId>,
    pub lyrics: Option<String>,
    /// Integrated loudness of the audio; on write it becomes `R128_TRACK_GAIN`
    /// (Opus) or `REPLAYGAIN_TRACK_GAIN` (everything else).
    pub loudness_lufs: Option<f64>,
    /// Read side: `R128_TRACK_GAIN`, Q7.8 dB relative to -23 LUFS.
    pub r128_track_gain: Option<i16>,
    /// Read side: `REPLAYGAIN_TRACK_GAIN` in dB.
    pub replaygain_track_gain_db: Option<f32>,
    /// Encoded image (JPEG/PNG) of the front cover.
    pub cover: Option<Vec<u8>>,
}

/// Technical facts about an audio file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    pub codec: Codec,
    pub duration: Duration,
    /// Average audio bitrate, kbps.
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
}

impl FileInfo {
    pub fn duration_ms(&self) -> u32 {
        u32::try_from(self.duration.as_millis()).unwrap_or(u32::MAX)
    }
}

/// `R128_TRACK_GAIN` value for a measured loudness: the gain that brings the track to -23 LUFS.
pub fn r128_gain_q8(lufs: f64) -> Option<i16> {
    if !lufs.is_finite() {
        return None;
    }
    let q8 = ((R128_REFERENCE_LUFS - lufs) * 256.0).round();
    Some(q8.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16)
}

/// `REPLAYGAIN_TRACK_GAIN` in dB (reference -18 LUFS).
pub fn replaygain_db(lufs: f64) -> Option<f32> {
    lufs.is_finite()
        .then(|| (REPLAYGAIN_REFERENCE_LUFS - lufs) as f32)
}

fn open_tagged(path: &Path) -> Result<TaggedFile> {
    lofty::read_from_path(crate::sanitize::long_path(path)).map_err(|e| ImportError::tags(path, e))
}

fn is_vorbis_family(ft: FileType) -> bool {
    matches!(ft, FileType::Opus | FileType::Vorbis | FileType::Flac)
}

/// Technical info only (no tags).
pub fn probe_info(path: &Path) -> Result<FileInfo> {
    let tagged = open_tagged(path)?;
    Ok(info_of(path, &tagged))
}

fn info_of(path: &Path, tagged: &TaggedFile) -> FileInfo {
    let p = tagged.properties();
    let ft = tagged.file_type();
    let codec = match ft {
        FileType::Flac => Codec::Flac,
        FileType::Opus => Codec::Opus,
        FileType::Vorbis => Codec::Vorbis,
        FileType::Mpeg => Codec::Mp3,
        FileType::Aac => Codec::Aac,
        // PCM in an AIFF container is lossless like WAV.
        FileType::Wav | FileType::Aiff => Codec::Wav,
        FileType::Mp4 => match mp4_codec(path) {
            Some(Mp4Codec::ALAC) => Codec::Alac,
            Some(Mp4Codec::FLAC) => Codec::Flac,
            Some(Mp4Codec::MP3) => Codec::Mp3,
            _ => Codec::Aac,
        },
        other => Codec::Other(format!("{other:?}").to_lowercase()),
    };
    FileInfo {
        codec,
        duration: p.duration(),
        bitrate_kbps: p.audio_bitrate().or_else(|| p.overall_bitrate()),
        // Opus always decodes at 48 kHz; the header's rate is only the input's.
        sample_rate: if ft == FileType::Opus { Some(48_000) } else { p.sample_rate() },
        bit_depth: p.bit_depth(),
        channels: p.channels(),
    }
}

fn mp4_codec(path: &Path) -> Option<Mp4Codec> {
    let mut f = File::open(crate::sanitize::long_path(path)).ok()?;
    Mp4File::read_from(&mut f, ParseOptions::new())
        .ok()?
        .properties()
        .codec()
}

fn text(tag: &Tag, key: ItemKey) -> Option<String> {
    tag.get_string(key)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(fix_cp1251_mojibake)
}

/// Old Russian MP3s carry Windows-1251 bytes in tags that claim to be Latin-1,
/// so "Кино" arrives as "Êèíî". When a string has no ASCII letters and its
/// non-ASCII characters are all the Latin-1 positions of Cyrillic letters, it
/// is read as Windows-1251 instead. Anything else is returned unchanged.
pub(crate) fn fix_cp1251_mojibake(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut letters = 0;
    for c in s.chars() {
        match c {
            // 0xC0..=0xFF: А..я in Windows-1251.
            '\u{C0}'..='\u{FF}' => {
                letters += 1;
                out.push(char::from_u32(0x0410 + (u32::from(c) - 0xC0)).unwrap_or(c));
            }
            '\u{A8}' => out.push('Ё'),
            '\u{B8}' => out.push('ё'),
            '\u{B9}' => out.push('№'),
            '\u{A0}' | '«' | '»' => out.push(c),
            c if c.is_ascii() && !c.is_ascii_alphabetic() => out.push(c),
            _ => return s.to_owned(),
        }
    }
    if letters >= 2 { out } else { s.to_owned() }
}

fn parse_year(s: &str) -> Option<i32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    if digits.len() == 4 {
        digits.parse().ok().filter(|y| (1..=9999).contains(y))
    } else {
        None
    }
}

/// Parses `"3"` and `"3/12"`.
fn parse_pair(s: &str) -> (Option<u32>, Option<u32>) {
    let mut it = s.split('/');
    let a = it.next().and_then(|x| x.trim().parse().ok());
    let b = it.next().and_then(|x| x.trim().parse().ok());
    (a.filter(|n| *n > 0), b.filter(|n| *n > 0))
}

fn parse_gain_db(s: &str) -> Option<f32> {
    s.trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
        .parse()
        .ok()
}

/// Reads tags and technical info in one go.
pub fn read_tags(path: &Path) -> Result<(TagSet, FileInfo)> {
    let tagged = open_tagged(path)?;
    let info = info_of(path, &tagged);
    let ft = tagged.file_type();
    let mut set = TagSet::default();
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        set.title = text(tag, ItemKey::TrackTitle);
        set.artists = tag
            .get_strings(ItemKey::TrackArtist)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(fix_cp1251_mojibake)
            .collect();
        set.artist = match set.artists.len() {
            0 => None,
            1 => Some(set.artists[0].clone()),
            _ => Some(set.artists.join(", ")),
        };
        set.album = text(tag, ItemKey::AlbumTitle);
        set.album_artist = text(tag, ItemKey::AlbumArtist);
        set.genre = text(tag, ItemKey::Genre);
        set.compilation = text(tag, ItemKey::FlagCompilation).is_some_and(|v| v == "1");

        let (no, total) = text(tag, ItemKey::TrackNumber)
            .map(|s| parse_pair(&s))
            .unwrap_or_default();
        set.track_no = no;
        set.track_total = text(tag, ItemKey::TrackTotal)
            .and_then(|s| s.trim().parse().ok())
            .or(total)
            .filter(|n| *n > 0);
        let (dno, dtotal) = text(tag, ItemKey::DiscNumber)
            .map(|s| parse_pair(&s))
            .unwrap_or_default();
        set.disc_no = dno;
        set.disc_total = text(tag, ItemKey::DiscTotal)
            .and_then(|s| s.trim().parse().ok())
            .or(dtotal)
            .filter(|n| *n > 0);

        set.date = text(tag, ItemKey::RecordingDate)
            .or_else(|| text(tag, ItemKey::ReleaseDate))
            .or_else(|| text(tag, ItemKey::Year));
        set.year = set
            .date
            .as_deref()
            .and_then(parse_year)
            .or_else(|| text(tag, ItemKey::OriginalReleaseDate).and_then(|d| parse_year(&d)));

        set.recording_mbid = text(tag, ItemKey::MusicBrainzRecordingId);
        set.release_mbid = text(tag, ItemKey::MusicBrainzReleaseId);
        set.release_group_mbid = text(tag, ItemKey::MusicBrainzReleaseGroupId);
        set.artist_mbid = text(tag, ItemKey::MusicBrainzArtistId);
        set.isrc = text(tag, ItemKey::Isrc);
        set.acoustid_id = text(tag, ItemKey::AcoustId);
        set.acoustid_fingerprint = text(tag, ItemKey::AcoustIdFingerprint);
        set.lyrics = text(tag, ItemKey::UnsyncLyrics).or_else(|| text(tag, ItemKey::Lyrics));
        set.r128_track_gain = text(tag, ItemKey::R128TrackGain).and_then(|s| s.trim().parse().ok());
        set.replaygain_track_gain_db =
            text(tag, ItemKey::ReplayGainTrackGain).and_then(|s| parse_gain_db(&s));

        let pics = tag.pictures();
        set.cover = pics
            .iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pics.first())
            .map(|p| p.data().to_vec());
    }
    set.track_id = read_track_id(path, ft);
    Ok((set, info))
}

/// Our track id from the file, if it carries one.
pub fn read_track_id(path: &Path, ft: FileType) -> Option<TrackId> {
    let p = crate::sanitize::long_path(path);
    let mut f = File::open(p).ok()?;
    let opts = ParseOptions::new();
    let raw: Option<String> = match ft {
        FileType::Opus => OpusFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()
            .get(TRACK_ID_KEY)
            .map(str::to_owned),
        FileType::Vorbis => VorbisFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()
            .get(TRACK_ID_KEY)
            .map(str::to_owned),
        FileType::Flac => FlacFile::read_from(&mut f, opts)
            .ok()?
            .vorbis_comments()
            .and_then(|v| v.get(TRACK_ID_KEY))
            .map(str::to_owned),
        FileType::Mpeg => MpegFile::read_from(&mut f, opts)
            .ok()?
            .id3v2()
            .and_then(|t| t.get_user_text(TRACK_ID_KEY))
            .map(str::to_owned),
        FileType::Mp4 => {
            let mp4 = Mp4File::read_from(&mut f, opts).ok()?;
            let ident = AtomIdent::Freeform {
                mean: MP4_MEAN.into(),
                name: TRACK_ID_KEY.into(),
            };
            mp4.ilst()?
                .get(&ident)?
                .data()
                .find_map(|d| match d {
                    AtomData::UTF8(s) | AtomData::UTF16(s) => Some(s.clone()),
                    _ => None,
                })
        }
        _ => None,
    };
    raw.and_then(|s| s.trim().parse().ok())
}

fn file_type_of(path: &Path) -> Result<FileType> {
    Ok(open_tagged(path)?.file_type())
}

/// Writes `set` into the file, replacing the fields it carries and keeping
/// everything else the file already had.
pub fn write_tags(path: &Path, set: &TagSet) -> Result<()> {
    let ft = file_type_of(path)?;
    if is_vorbis_family(ft) {
        write_vorbis(path, ft, set)
    } else {
        write_generic(path, ft, set)
    }
}

fn read_vorbis_comments(path: &Path, ft: FileType) -> Result<VorbisComments> {
    let mut f = File::open(crate::sanitize::long_path(path)).map_err(|e| ImportError::io(path, e))?;
    let opts = ParseOptions::new();
    let bad = |e: lofty::error::FileParseError| ImportError::tags(path, e);
    Ok(match ft {
        FileType::Opus => OpusFile::read_from(&mut f, opts)
            .map_err(bad)?
            .vorbis_comments()
            .clone(),
        FileType::Vorbis => VorbisFile::read_from(&mut f, opts)
            .map_err(bad)?
            .vorbis_comments()
            .clone(),
        _ => FlacFile::read_from(&mut f, opts)
            .map_err(bad)?
            .vorbis_comments()
            .cloned()
            .unwrap_or_default(),
    })
}

fn put(vc: &mut VorbisComments, key: &str, value: Option<&str>) {
    if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
        vc.insert(key.to_owned(), v.to_owned());
    }
}

fn picture_of(bytes: &[u8]) -> Picture {
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        MimeType::Png
    } else {
        MimeType::Jpeg
    };
    Picture::unchecked(bytes.to_vec())
        .pic_type(PictureType::CoverFront)
        .mime_type(mime)
        .build()
}

fn write_vorbis(path: &Path, ft: FileType, t: &TagSet) -> Result<()> {
    let mut vc = read_vorbis_comments(path, ft)?;
    put(&mut vc, "TITLE", t.title.as_deref());
    if !t.artists.is_empty() {
        for _ in vc.remove("ARTIST") {}
        for a in &t.artists {
            vc.push("ARTIST".into(), a.clone());
        }
    } else {
        put(&mut vc, "ARTIST", t.artist.as_deref());
    }
    put(&mut vc, "ALBUM", t.album.as_deref());
    put(&mut vc, "ALBUMARTIST", t.album_artist.as_deref());
    put(&mut vc, "GENRE", t.genre.as_deref());
    if let Some(n) = t.track_no {
        put(&mut vc, "TRACKNUMBER", Some(&n.to_string()));
    }
    // The other spellings of the counts go: one count, read back as written.
    if let Some(n) = t.track_total {
        for _ in vc.remove("TOTALTRACKS") {}
        put(&mut vc, "TRACKTOTAL", Some(&n.to_string()));
    }
    if let Some(n) = t.disc_no {
        put(&mut vc, "DISCNUMBER", Some(&n.to_string()));
    }
    if let Some(n) = t.disc_total {
        for _ in vc.remove("TOTALDISCS") {}
        put(&mut vc, "DISCTOTAL", Some(&n.to_string()));
    }
    let date = t.date.clone().or_else(|| t.year.map(|y| format!("{y:04}")));
    put(&mut vc, "DATE", date.as_deref());
    if t.compilation {
        put(&mut vc, "COMPILATION", Some("1"));
    }
    put(&mut vc, "MUSICBRAINZ_TRACKID", t.recording_mbid.as_deref());
    put(&mut vc, "MUSICBRAINZ_ALBUMID", t.release_mbid.as_deref());
    put(
        &mut vc,
        "MUSICBRAINZ_RELEASEGROUPID",
        t.release_group_mbid.as_deref(),
    );
    put(&mut vc, "MUSICBRAINZ_ARTISTID", t.artist_mbid.as_deref());
    put(&mut vc, "ISRC", t.isrc.as_deref());
    put(&mut vc, "ACOUSTID_ID", t.acoustid_id.as_deref());
    put(
        &mut vc,
        "ACOUSTID_FINGERPRINT",
        t.acoustid_fingerprint.as_deref(),
    );
    put(&mut vc, "UNSYNCEDLYRICS", t.lyrics.as_deref());
    if let Some(id) = t.track_id {
        put(&mut vc, TRACK_ID_KEY, Some(&id.to_string()));
    }
    if let Some(lufs) = t.loudness_lufs {
        if ft == FileType::Opus {
            if let Some(q8) = r128_gain_q8(lufs) {
                put(&mut vc, "R128_TRACK_GAIN", Some(&q8.to_string()));
            }
        } else if let Some(db) = replaygain_db(lufs) {
            put(
                &mut vc,
                "REPLAYGAIN_TRACK_GAIN",
                Some(&format!("{db:.2} dB")),
            );
            put(&mut vc, "REPLAYGAIN_REFERENCE_LOUDNESS", Some("-18.00 LUFS"));
        }
    }
    if let Some(cover) = &t.cover {
        vc.remove_picture_type(PictureType::CoverFront);
        vc.insert_picture(picture_of(cover), None)
            .map_err(|e| ImportError::tags(path, e))?;
    }
    vc.save_to_path(crate::sanitize::long_path(path), WriteOptions::default())
        .map_err(|e| ImportError::tags(path, e))
}

fn set_text(tag: &mut Tag, key: ItemKey, value: Option<&str>) {
    if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
        tag.insert_text(key, v.to_owned());
    }
}

fn write_generic(path: &Path, ft: FileType, t: &TagSet) -> Result<()> {
    let mut tagged = open_tagged(path)?;
    let tag_type = tagged.primary_tag_type();
    if tagged.tag(tag_type).is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged
        .tag_mut(tag_type)
        .ok_or_else(|| ImportError::tags(path, "no writable tag"))?;
    set_text(tag, ItemKey::TrackTitle, t.title.as_deref());
    set_text(tag, ItemKey::TrackArtist, t.artist.as_deref());
    set_text(tag, ItemKey::AlbumTitle, t.album.as_deref());
    set_text(tag, ItemKey::AlbumArtist, t.album_artist.as_deref());
    set_text(tag, ItemKey::Genre, t.genre.as_deref());
    if let Some(n) = t.track_no {
        tag.insert_text(ItemKey::TrackNumber, n.to_string());
    }
    if let Some(n) = t.track_total {
        tag.insert_text(ItemKey::TrackTotal, n.to_string());
    }
    if let Some(n) = t.disc_no {
        tag.insert_text(ItemKey::DiscNumber, n.to_string());
    }
    if let Some(n) = t.disc_total {
        tag.insert_text(ItemKey::DiscTotal, n.to_string());
    }
    let date = t.date.clone().or_else(|| t.year.map(|y| format!("{y:04}")));
    set_text(tag, ItemKey::RecordingDate, date.as_deref());
    if t.compilation {
        tag.insert_text(ItemKey::FlagCompilation, "1".to_owned());
    }
    set_text(
        tag,
        ItemKey::MusicBrainzRecordingId,
        t.recording_mbid.as_deref(),
    );
    set_text(tag, ItemKey::MusicBrainzReleaseId, t.release_mbid.as_deref());
    set_text(
        tag,
        ItemKey::MusicBrainzReleaseGroupId,
        t.release_group_mbid.as_deref(),
    );
    set_text(tag, ItemKey::MusicBrainzArtistId, t.artist_mbid.as_deref());
    set_text(tag, ItemKey::Isrc, t.isrc.as_deref());
    set_text(tag, ItemKey::AcoustId, t.acoustid_id.as_deref());
    set_text(
        tag,
        ItemKey::AcoustIdFingerprint,
        t.acoustid_fingerprint.as_deref(),
    );
    set_text(tag, ItemKey::UnsyncLyrics, t.lyrics.as_deref());
    if let Some(db) = t.loudness_lufs.and_then(replaygain_db) {
        tag.insert_text(ItemKey::ReplayGainTrackGain, format!("{db:.2} dB"));
    }
    if let Some(cover) = &t.cover {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(picture_of(cover));
    }
    tagged
        .save_to_path(crate::sanitize::long_path(path), WriteOptions::default())
        .map_err(|e| ImportError::tags(path, e))?;
    if let Some(id) = t.track_id {
        write_custom_track_id(path, ft, id)?;
    }
    Ok(())
}

fn write_custom_track_id(path: &Path, ft: FileType, id: TrackId) -> Result<()> {
    let p = crate::sanitize::long_path(path);
    let opts = ParseOptions::new();
    let bad = |e: lofty::error::FileParseError| ImportError::tags(path, e);
    let save_bad = |e: lofty::error::FileEncodingError| ImportError::tags(path, e);
    match ft {
        FileType::Mpeg => {
            let mut f = File::open(&p).map_err(|e| ImportError::io(path, e))?;
            let mut mpeg = MpegFile::read_from(&mut f, opts).map_err(bad)?;
            if mpeg.id3v2().is_none() {
                mpeg.set_id3v2(Id3v2Tag::new());
            }
            if let Some(id3) = mpeg.id3v2_mut() {
                id3.insert_user_text(TRACK_ID_KEY.to_owned(), id.to_string());
            }
            mpeg.save_to_path(&p, WriteOptions::default())
                .map_err(save_bad)
        }
        FileType::Mp4 => {
            let mut f = File::open(&p).map_err(|e| ImportError::io(path, e))?;
            let mut mp4 = Mp4File::read_from(&mut f, opts).map_err(bad)?;
            if let Some(ilst) = mp4.ilst_mut() {
                ilst.replace_atom(Atom::new(
                    AtomIdent::Freeform {
                        mean: MP4_MEAN.into(),
                        name: TRACK_ID_KEY.into(),
                    },
                    AtomData::UTF8(id.to_string()),
                ));
            }
            mp4.save_to_path(&p, WriteOptions::default())
                .map_err(save_bad)
        }
        other => {
            log::warn!(
                "{}: no place for {TRACK_ID_KEY} in {other:?} files; the DB cannot be rebuilt for this track",
                path.display()
            );
            Ok(())
        }
    }
}

/// A tag field that [`remove_fields`] can delete. [`write_tags`] only ever sets
/// values; clearing a field in the tag editor needs this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Album,
    AlbumArtist,
    Genre,
    /// Every date field (`DATE`, `YEAR`, `ORIGINALDATE`, ...).
    Date,
    TrackNumber,
    DiscNumber,
    RecordingMbid,
    ReleaseMbid,
}

impl Field {
    fn vorbis_keys(self) -> &'static [&'static str] {
        match self {
            Self::Album => &["ALBUM"],
            Self::AlbumArtist => &["ALBUMARTIST"],
            Self::Genre => &["GENRE"],
            Self::Date => &["DATE", "YEAR", "RELEASEDATE", "ORIGINALDATE"],
            Self::TrackNumber => &["TRACKNUMBER"],
            Self::DiscNumber => &["DISCNUMBER"],
            Self::RecordingMbid => &["MUSICBRAINZ_TRACKID"],
            Self::ReleaseMbid => &["MUSICBRAINZ_ALBUMID"],
        }
    }

    fn item_keys(self) -> &'static [ItemKey] {
        match self {
            Self::Album => &[ItemKey::AlbumTitle],
            Self::AlbumArtist => &[ItemKey::AlbumArtist],
            Self::Genre => &[ItemKey::Genre],
            Self::Date => &[
                ItemKey::RecordingDate,
                ItemKey::ReleaseDate,
                ItemKey::Year,
                ItemKey::OriginalReleaseDate,
            ],
            Self::TrackNumber => &[ItemKey::TrackNumber],
            Self::DiscNumber => &[ItemKey::DiscNumber],
            Self::RecordingMbid => &[ItemKey::MusicBrainzRecordingId],
            Self::ReleaseMbid => &[ItemKey::MusicBrainzReleaseId],
        }
    }
}

/// Deletes `fields` from the file's tags, keeping everything else.
pub fn remove_fields(path: &Path, fields: &[Field]) -> Result<()> {
    if fields.is_empty() {
        return Ok(());
    }
    let ft = file_type_of(path)?;
    if is_vorbis_family(ft) {
        let mut vc = read_vorbis_comments(path, ft)?;
        for key in fields.iter().flat_map(|f| f.vorbis_keys()) {
            for _ in vc.remove(key) {}
        }
        return vc
            .save_to_path(crate::sanitize::long_path(path), WriteOptions::default())
            .map_err(|e| ImportError::tags(path, e));
    }
    let mut tagged = open_tagged(path)?;
    let tag_type = tagged.primary_tag_type();
    let Some(tag) = tagged.tag_mut(tag_type) else {
        return Ok(());
    };
    for key in fields.iter().flat_map(|f| f.item_keys()) {
        tag.remove_key(*key);
    }
    tagged
        .save_to_path(crate::sanitize::long_path(path), WriteOptions::default())
        .map_err(|e| ImportError::tags(path, e))
}

/// Replaces only the lyrics of an already tagged file.
pub fn write_lyrics(path: &Path, lyrics: &str) -> Result<()> {
    write_tags(
        path,
        &TagSet {
            lyrics: Some(lyrics.to_owned()),
            ..TagSet::default()
        },
    )
}

/// Re-reads the file type (used by tests and the rebuild).
pub fn file_type(path: &Path) -> Result<FileType> {
    file_type_of(path)
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
