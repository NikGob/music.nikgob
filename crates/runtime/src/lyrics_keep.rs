//! Where a track's lyrics are made from and kept (docs/lyrics-align.md,
//! «Keeping a text»): the audio timing is made from (the track's file),
//! and «Сохранить текст», which puts the shown text with the player's
//! timing on the computer: into the file's lyrics tag (enhanced LRC) and a
//! `.ttml` next to the file, which the player reads first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource};
use nikgob_core::model::TrackId;

use crate::Worker;
use crate::align::Msg;
use crate::import::Internal;
use crate::lyrics::{Place, Text};

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
        match self.lib.active_source(track).ok().flatten() {
            Some(src) if src.kind.is_local() => Place::File,
            _ => Place::None,
        }
    }

    /// `Command::SaveLyrics`: the shown text with the player's timing goes
    /// onto the computer (see the module docs).
    pub(crate) fn save_lyrics(&mut self, track: TrackId, source: LyricsSource) {
        let Some(tl) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)) else { return };
        let Some(lines) = tl.shown_lines(source).map(|l| to_millis(&l)) else { return };
        if tl.place != Place::File {
            return;
        }
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
        let file = self.res.path(&src.locator);
        self.keep_next_to(track, file, lines);
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
        let tl = crate::lyrics::cached_lyrics(&lib, track, Some(&file), Some(&tags), false);
        assert_eq!(tl.texts_of(LyricsSource::File).map(|t| &t.lines), Some(&lines), "the .ttml, not the tag");
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
