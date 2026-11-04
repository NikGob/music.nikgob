//! Where a track's lyrics are made from and kept (docs/lyrics-align.md,
//! «Keeping a text»): the audio timing is made from (the track's file, or
//! a stream fetched for a while when its artist and song are clear), and
//! «Сохранить текст», which puts the shown text with the player's timing
//! on the computer: into the file's lyrics tag (enhanced LRC) and a
//! `.ttml` next to the file, which the player reads first. A song without
//! a file keeps the `.ttml` in its artist's folder of the library («Только
//! текст»), or is saved as a file there first and keeps its text next to
//! it like any song («Песню и текст»).

use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource};
use nikgob_core::model::{SourceKind, TrackId, score};
use nikgob_core::streaming::Service;

use crate::Worker;
use crate::align::Msg;
use crate::import::Internal;
use crate::lyrics::{Place, Text};

/// Why a stream's timing is not made: what plays is a clip, whose song is
/// not clear.
pub(crate) const CLIP: &str = "Подсветку не сделать: это клип, неясно, какая в нём песня";
/// … an upload that is not the artist's own.
pub(crate) const UPLOAD: &str = "Подсветку не сделать: неофициальная загрузка, неясно, чья это песня";

/// Where a stream's timing can come from: an official song (not a clip)
/// on an album, or anything whose recording the library knows otherwise
/// (an ISRC, a MusicBrainz id, another service's entry). Covers are songs
/// like any.
pub(crate) fn stream_place(clip: bool, album: bool, known: bool) -> Place {
    if known || (album && !clip) {
        Place::Stream
    } else if clip {
        Place::Unclear(CLIP.into())
    } else {
        Place::Unclear(UPLOAD.into())
    }
}

/// `lines` with their times to the millisecond (as the files keep them),
/// so what is shown after saving is what was saved.
pub(crate) fn to_millis(lines: &[LyricLine]) -> Vec<LyricLine> {
    let ms = |d: Duration| Duration::from_millis(d.as_millis() as u64);
    lines
        .iter()
        .map(|l| LyricLine {
            start: ms(l.start),
            end: l.end.map(ms),
            words: l.words.iter().map(|w| LyricWord { start: ms(w.start), end: ms(w.end), text: w.text.clone() }).collect(),
            ..l.clone()
        })
        .collect()
}

/// Writes the `.ttml` at `path` (its folder made when missing) and gives
/// the text as the player reads it back.
pub(crate) fn write_ttml(path: &Path, lines: &[LyricLine]) -> Result<Text, String> {
    let xml = nikgob_metadata::serialize_ttml(lines);
    let long = nikgob_import::sanitize::long_path(path);
    if let Some(dir) = long.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&long, &xml).map_err(|e| e.to_string())?;
    Text::from_ttml(&xml).ok_or_else(|| "пустой текст".to_owned())
}

/// The lyrics tag of `file` as enhanced LRC (word stamps where the timing
/// has words), unless it already says so.
pub(crate) fn write_tag(file: &Path, lines: &[LyricLine]) -> Result<(), String> {
    crate::lyrics::embed_text(file, &nikgob_metadata::lrc::serialize_lrc(lines)).map_err(|e| e.to_string())
}

impl Worker {
    /// Where `track`'s timing can be made from.
    pub(crate) fn lyrics_place(&self, track: TrackId) -> Place {
        let Some(src) = self.lib.active_source(track).ok().flatten() else { return Place::None };
        if src.kind.is_local() {
            return Place::File;
        }
        if src.kind != SourceKind::Youtube || !self.state.read().prefs.lyrics_streams {
            return Place::None;
        }
        let t = self.lib.get_track(track).ok().flatten();
        let elsewhere = self
            .lib
            .sources_of_track(track)
            .unwrap_or_default()
            .iter()
            .any(|s| s.kind != SourceKind::Youtube && s.trashed_at.is_none());
        let known = elsewhere || t.as_ref().is_some_and(|t| t.isrc.is_some() || t.recording_mbid.is_some());
        let album = t.as_ref().is_some_and(|t| t.release_id.is_some());
        stream_place(src.quality_score == score::STREAM_YOUTUBE_CLIP, album, known)
    }

    /// Where the text of `track` is kept while it has no file: its
    /// artist's folder of the library.
    pub(crate) fn kept_text(&self, track: TrackId) -> Option<PathBuf> {
        let dir = self.import.as_ref()?.config().library_dir.clone();
        let t = self.lib.get_track(track).ok().flatten()?;
        let artist = t.artist_credit.0.first().map(|p| p.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| t.artist_credit.display());
        Some(nikgob_import::layout::kept_text_path(&dir, &artist, &t.title))
    }

    /// `Command::SaveLyrics`: the shown text with the player's timing goes
    /// onto the computer (see the module docs).
    pub(crate) fn save_lyrics(&mut self, track: TrackId, source: LyricsSource) {
        let Some(tl) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)) else { return };
        let Some(lines) = tl.shown_lines(source).map(|l| to_millis(&l)) else { return };
        match tl.place.clone() {
            Place::File => {
                let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
                let file = self.res.path(&src.locator);
                self.keep_next_to(track, file, lines);
            }
            Place::Stream if self.state.read().prefs.lyrics_stream_keep == 1 => self.keep_with_song(track, lines),
            Place::Stream => {
                let Some(path) = self.kept_text(track) else { return };
                let Some(svc) = self.import.as_ref() else { return };
                let tx = svc.internal();
                svc.spawn_blocking(move || {
                    let result = write_ttml(&path, &lines);
                    let _ = tx.send(Internal::Align(Msg::Kept { track, result }));
                });
            }
            Place::None | Place::Unclear(_) => {}
        }
    }

    /// The `.ttml` next to `file` at once, the file's tag once the file is
    /// not playing (rewriting it under the decoder shifts its pages).
    fn keep_next_to(&mut self, track: TrackId, file: PathBuf, lines: Vec<LyricLine>) {
        let in_use = |e: Option<crate::queue::Entry>| e.is_some_and(|e| e.track == track);
        let later = in_use(self.queue.current()) || in_use(self.queue.peek_auto_next());
        if later {
            self.align.tag_later.insert(track, (file.clone(), lines.clone()));
        }
        let Some(svc) = self.import.as_ref() else { return };
        let tx = svc.internal();
        svc.spawn_blocking(move || {
            let result = write_ttml(&file.with_extension("ttml"), &lines).and_then(|text| {
                if !later {
                    write_tag(&file, &lines)?;
                }
                Ok(text)
            });
            let _ = tx.send(Internal::Align(Msg::Kept { track, result }));
        });
    }

    /// Tags left to write for files that were playing (see
    /// [`Self::keep_next_to`]); called when another track starts.
    pub(crate) fn write_later_tags(&mut self) {
        let busy = [self.queue.current().map(|e| e.track), self.queue.peek_auto_next().map(|e| e.track)];
        let ready: Vec<TrackId> = self.align.tag_later.keys().copied().filter(|t| !busy.contains(&Some(*t))).collect();
        let Some(svc) = self.import.as_ref() else { return };
        for t in ready {
            let Some((file, lines)) = self.align.tag_later.remove(&t) else { continue };
            svc.spawn_blocking(move || {
                if let Err(e) = write_tag(&file, &lines) {
                    log::warn!("saving lyrics into {}: {e}", file.display());
                }
            });
        }
    }

    /// «Песню и текст»: the stream's audio is saved and imported as the
    /// track's file (as «Сохранить с YouTube» does), then the text goes
    /// next to it ([`Self::lyrics_imported`]).
    fn keep_with_song(&mut self, track: TrackId, lines: Vec<LyricLine>) {
        let Some(yt) = self.service(Service::Youtube) else {
            self.toast("YouTube Music выключен", None);
            return;
        };
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind == SourceKind::Youtube) else { return };
        self.align.after_import.insert(track, lines);
        let dir = self.save_dir();
        let locator = src.locator.clone();
        self.streaming.spawn_internal(async move {
            let result = yt.save_audio(&locator, &dir).await.map_err(|e| e.to_string());
            Internal::Align(Msg::SongSaved { track, result })
        });
        self.toast("Сохраняю песню с YouTube…", None);
    }

    pub(crate) fn song_saved(&mut self, track: TrackId, result: Result<PathBuf, String>) {
        match result {
            Ok(path) => {
                self.align.saved_audio.insert(path.clone(), track);
                let targets = nikgob_import::ImportTargets { tracks: vec![track], force: true, ..Default::default() };
                if !self.import_for(vec![path], "Сохранение с YouTube".into(), nikgob_core::model::SourceOrigin::Stream, targets) {
                    self.align.after_import.remove(&track);
                }
            }
            Err(e) => {
                log::warn!("saving a song for its lyrics: {e}");
                self.align.after_import.remove(&track);
                self.toast(format!("Не получилось сохранить песню: {e}"), None);
            }
        }
    }

    /// A file landed: a song saved for its text gets the text next to it.
    pub(crate) fn lyrics_imported(&mut self, track: TrackId, path: &Path) {
        if self.align.saved_audio.remove(path).is_some() {
            let _ = std::fs::remove_file(path);
        }
        let Some(lines) = self.align.after_import.remove(&track) else { return };
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
        let file = self.res.path(&src.locator);
        if let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) {
            tl.place = Place::File;
        }
        self.keep_next_to(track, file, lines);
    }

    /// The text is on the computer: it is the track's own text now, shown
    /// as the player picks it (a file's timed text wins).
    pub(crate) fn lyrics_kept(&mut self, track: TrackId, result: Result<Text, String>) {
        let text = match result {
            Ok(t) => t,
            Err(e) => {
                log::warn!("saving lyrics: {e}");
                self.toast(format!("Не получилось сохранить текст: {e}"), None);
                return;
            }
        };
        if let Err(e) = self.lib.set_lyrics_pick(track, None) {
            log::warn!("saving lyrics pick: {e}");
        }
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        tl.set_text(LyricsSource::File, Some(text));
        tl.set_pick(None);
        let view = tl.view(track);
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_is_timed_only_when_its_song_is_clear() {
        assert_eq!(stream_place(false, true, false), Place::Stream, "an official song on an album");
        assert_eq!(stream_place(true, false, true), Place::Stream, "a clip of a song the library knows");
        assert_eq!(stream_place(true, false, false), Place::Unclear(CLIP.into()));
        assert_eq!(stream_place(true, true, false), Place::Unclear(CLIP.into()), "a clip on an album stays unclear");
        assert_eq!(stream_place(false, false, false), Place::Unclear(UPLOAD.into()));
    }

    fn opus(path: &Path) {
        let wav = path.with_extension("wav");
        let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for _ in 0..48_000 {
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
        use nikgob_import::Encoder;
        nikgob_import::RopusEncoder.encode_to_opus(&wav, path, &nikgob_import::EncodeOptions { bitrate_kbps: 96 }).unwrap();
        std::fs::remove_file(wav).unwrap();
    }

    fn timed() -> Vec<LyricLine> {
        let w = |a: u64, b: u64, t: &str| LyricWord { start: Duration::from_millis(a), end: Duration::from_millis(b), text: t.into() };
        let mut first = LyricLine::new(Duration::from_millis(1_000), "Знаешь, не беда");
        first.words = vec![w(1_000, 1_400, "Знаешь, "), w(1_400, 1_600, "не "), w(1_600, 4_200, "беда")];
        first.end = Some(Duration::from_millis(4_200));
        let mut bg = LyricLine::new(Duration::from_millis(3_000), "не беда");
        bg.words = vec![w(3_000, 3_300, "не "), w(3_300, 3_900, "беда")];
        bg.end = Some(Duration::from_millis(3_900));
        bg.background = true;
        let plain = LyricLine::new(Duration::from_millis(5_000), "Строка без слов");
        vec![first, bg, plain]
    }

    /// «Сохранить текст» for a song with a file: the tag holds enhanced
    /// LRC (word stamps), the `.ttml` next to it every word and the
    /// background line, and the player reads the `.ttml` first.
    #[test]
    fn a_kept_text_goes_into_the_tag_and_a_ttml_the_player_reads_first() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Song.opus");
        opus(&file);
        let lines = to_millis(&timed());
        let text = write_ttml(&file.with_extension("ttml"), &lines).unwrap();
        write_tag(&file, &lines).unwrap();
        assert_eq!(text.lines, lines, "read back as saved");
        let (tags, _) = nikgob_import::tags::read_tags(&file).unwrap();
        let tag = tags.lyrics.clone().unwrap();
        assert!(tag.contains("<00:01.600>беда"), "{tag}");
        let main: Vec<LyricLine> = nikgob_metadata::parse_lrc_lines(&tag).into_iter().filter(|l| l.text != "не беда").collect();
        assert_eq!(main[0].words.iter().map(|w| w.start).collect::<Vec<_>>(), lines[0].words.iter().map(|w| w.start).collect::<Vec<_>>());
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&nikgob_core::model::Track::new("Song", nikgob_core::model::ArtistCredit::single("Artist", None))).unwrap().id;
        let tl = crate::lyrics::cached_lyrics(&lib, track, Some(&file), Some(&tags), false, None);
        assert_eq!(tl.texts_of(LyricsSource::File).map(|t| &t.lines), Some(&lines), "the .ttml, not the tag");
    }

    /// «Только текст» for a song without a file: the `.ttml` in its
    /// artist's folder of the library (a temp one here) is its text.
    #[test]
    fn a_stream_s_kept_text_is_found_again_in_its_artist_s_folder() {
        let root = tempfile::tempdir().unwrap();
        let path = nikgob_import::layout::kept_text_path(root.path(), "LAZZY2WICE", "Summer 15/16");
        assert!(path.starts_with(root.path()));
        let lines = to_millis(&timed());
        write_ttml(&path, &lines).unwrap();
        assert!(root.path().join("LAZZY2WICE").join("Summer 15-16.ttml").exists());
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&nikgob_core::model::Track::new("Summer 15/16", nikgob_core::model::ArtistCredit::single("LAZZY2WICE", None))).unwrap().id;
        let tl = crate::lyrics::cached_lyrics(&lib, track, None, None, false, Some(&path));
        assert_eq!(tl.texts_of(LyricsSource::File).map(|t| &t.lines), Some(&lines));
        // A song with its own file reads only what lies next to the file.
        let file = root.path().join("Song.opus");
        let other = crate::lyrics::cached_lyrics(&lib, track, Some(&file), None, false, Some(&path));
        assert!(other.texts_of(LyricsSource::File).is_none());
    }

    #[test]
    fn kept_times_are_whole_milliseconds() {
        let mut l = LyricLine::new(Duration::from_secs_f32(1.234_567), "a");
        l.words.push(LyricWord { start: Duration::from_secs_f32(1.234_567), end: Duration::from_secs_f32(2.000_9), text: "a".into() });
        l.end = Some(Duration::from_secs_f32(2.000_9));
        let m = &to_millis(&[l])[0];
        assert_eq!(m.start, Duration::from_millis(1234));
        assert_eq!(m.words[0].end, Duration::from_millis(2000));
        assert_eq!(m.end, Some(Duration::from_millis(2000)));
    }
}
