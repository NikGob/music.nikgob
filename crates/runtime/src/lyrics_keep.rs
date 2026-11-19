//! Where a track's lyrics are made from and kept: the audio timing is made from (the track's file, or
//! a stream fetched for a while when its artist and song are clear), and
//! «Сохранить текст», which puts the shown text with its timing (a
//! source's, or the one the player made; none for a plain text) on the
//! computer: into the file's lyrics tag (enhanced LRC, LRC or the plain
//! text) and a `.ttml` next to the file, which the player reads first. A song without
//! a file keeps the `.ttml` in its artist's folder of the library («Только
//! текст»), or is saved as a file there first and keeps its text next to
//! it like any song («Песню и текст»).
//!
//! The user's own text (edited by hand, «Изменить текст…») goes to the
//! same `.ttml` place, marked as theirs ([`nikgob_metadata::mark_ttml_own`]),
//! never into the tag: the player shows it first from then on. A text that
//! lay there before goes to the library's Trash, and so does the user's own
//! text when they go back to the found ones («Вернуть найденный», with
//! undo).

use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::i18n::{Locale, activity_in};
use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource};
use nikgob_core::model::{SourceKind, Track, TrackId};
use nikgob_core::streaming::Service;

use crate::Worker;
use crate::align::Msg;
use crate::import::Internal;
use crate::lyrics::{Place, Text};

/// Why a stream's timing is not made: the track has no artist or no
/// title to keep its text under.
pub(crate) fn unnamed(loc: Locale) -> &'static str {
    activity_in(loc).unnamed_highlight
}

/// Where a stream's timing can come from: any stream with an artist and a
/// title (`named`): a song YouTube Music has as a record, a cover like any
/// song, and a music video that stays one (no song was found for it): its
/// own audio is what plays, intro and all, so timing made from it fits it.
pub(crate) fn stream_place(named: bool, loc: Locale) -> Place {
    if named { Place::Stream } else { Place::Unclear(unnamed(loc).into()) }
}

/// The artist whose folder keeps a track's text: the first credited.
fn artist_of(t: &Track) -> String {
    t.artist_credit.0.first().map(|p| p.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| t.artist_credit.display())
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

/// What «Сохранить текст» puts on the computer: the shown text with its
/// finest timing, or a plain text as it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Keep {
    Lines(Vec<LyricLine>),
    Plain(String),
}

impl Keep {
    /// The user's own text as the editor gives it: `lines` with their
    /// timing, or (`timed` false) their texts one a line as a plain text.
    pub(crate) fn own(lines: Vec<LyricLine>, timed: bool) -> Self {
        if timed && !lines.is_empty() {
            Self::Lines(lines)
        } else {
            Self::Plain(lines.iter().map(|l| l.text.trim()).collect::<Vec<_>>().join("\n"))
        }
    }

    /// Nothing to keep: no line with a word in it.
    fn is_empty(&self) -> bool {
        match self {
            Self::Lines(l) => l.iter().all(|l| l.text.trim().is_empty()),
            Self::Plain(t) => t.trim().is_empty(),
        }
    }

    /// Times to the millisecond ([`to_millis`]).
    pub(crate) fn to_millis(self) -> Self {
        match self {
            Self::Lines(l) => Self::Lines(to_millis(&l)),
            plain => plain,
        }
    }

    /// As a `.ttml`: word or line timing, or none.
    fn ttml(&self) -> String {
        match self {
            Self::Lines(l) => nikgob_metadata::serialize_ttml(l),
            Self::Plain(t) => nikgob_metadata::serialize_ttml_plain(t),
        }
    }

    /// As the file's lyrics tag: enhanced LRC, LRC or the plain text.
    fn tag(&self) -> String {
        match self {
            Self::Lines(l) => nikgob_metadata::lrc::serialize_lrc(l),
            Self::Plain(t) => t.clone(),
        }
    }
}

/// Writes the `.ttml` at `path` (its folder made when missing) and gives
/// the text as the player reads it back.
pub(crate) fn write_ttml(path: &Path, keep: &Keep, loc: Locale) -> Result<Text, String> {
    write_xml(path, &keep.ttml(), loc)
}

/// [`write_ttml`] of the user's own text: marked as theirs.
pub(crate) fn write_own_ttml(path: &Path, keep: &Keep, loc: Locale) -> Result<Text, String> {
    write_xml(path, &nikgob_metadata::mark_ttml_own(&keep.ttml()), loc)
}

/// `xml` at `path` (nothing written for an empty text).
fn write_xml(path: &Path, xml: &str, loc: Locale) -> Result<Text, String> {
    let text = Text::from_ttml(xml).ok_or_else(|| activity_in(loc).empty_text.to_owned())?;
    let long = nikgob_import::sanitize::long_path(path);
    if let Some(dir) = long.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&long, xml).map_err(|e| e.to_string())?;
    Ok(text)
}

/// The user's own text at `path` ([`write_own_ttml`]), whatever lay there
/// before going to the Trash in `trash` first (a text «Сохранить текст»
/// put there: kept, not overwritten).
pub(crate) fn keep_own_at(path: &Path, trash: &Path, keep: &Keep, loc: Locale) -> Result<Text, String> {
    if keep.is_empty() {
        return Err(activity_in(loc).empty_text.to_owned());
    }
    let there = nikgob_import::sanitize::long_path(path).is_file();
    if there && !crate::lyrics::read_ttml(path).is_some_and(|(_, own)| own) {
        nikgob_import::trash_file(trash, path).map_err(|e| e.to_string())?;
    }
    write_own_ttml(path, keep, loc)
}

/// The user's own text at `path` goes to the Trash in `trash`; where it
/// went (`None`: nothing of theirs lies there).
pub(crate) fn drop_own_at(path: &Path, trash: &Path) -> Result<Option<PathBuf>, String> {
    if !crate::lyrics::read_ttml(path).is_some_and(|(_, own)| own) {
        return Ok(None);
    }
    nikgob_import::trash_file(trash, path).map(Some).map_err(|e| e.to_string())
}

/// Brings the user's own text back from the Trash (`at`) to `to`, unless
/// something lies there now.
pub(crate) fn restore_own_at(at: &Path, to: &Path, loc: Locale) -> Result<Text, String> {
    if nikgob_import::sanitize::long_path(to).exists() {
        return Err(activity_in(loc).other_text_there.to_owned());
    }
    if let Some(dir) = nikgob_import::sanitize::long_path(to).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    nikgob_import::fsutil::move_file(at, to).map_err(|e| e.to_string())?;
    crate::lyrics::read_ttml(to).map(|(t, _)| t).ok_or_else(|| activity_in(loc).empty_text.to_owned())
}

/// The lyrics tag of `file` as enhanced LRC (word stamps where the timing
/// has words), LRC or the plain text, unless it already says so.
pub(crate) fn write_tag(file: &Path, keep: &Keep) -> Result<(), String> {
    crate::lyrics::embed_text(file, &keep.tag()).map_err(|e| e.to_string())
}

impl Worker {
    /// The YouTube copy whose audio a stream's timing is made from: the
    /// active source, or for a track of another service (Spotify, whose
    /// audio cannot be saved) its YouTube copy that plays.
    pub(crate) fn timing_stream(&self, track: TrackId) -> Option<nikgob_core::model::Source> {
        let active = self.lib.active_source(track).ok().flatten()?;
        if active.kind == SourceKind::Youtube || active.kind.is_local() {
            return Some(active);
        }
        let mut copies: Vec<_> = self
            .lib
            .sources_of_track(track)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.kind == SourceKind::Youtube && s.trashed_at.is_none() && s.available)
            .collect();
        copies.sort_by_key(|s| std::cmp::Reverse(s.quality_score));
        copies.into_iter().next()
    }

    /// Where `track`'s timing can be made from.
    pub(crate) fn lyrics_place(&self, track: TrackId) -> Place {
        let Some(src) = self.timing_stream(track) else { return Place::None };
        if src.kind.is_local() {
            return Place::File;
        }
        if src.kind != SourceKind::Youtube || !self.state.read().prefs.lyrics_streams {
            return Place::None;
        }
        let t = self.lib.get_track(track).ok().flatten();
        let named = t.as_ref().is_some_and(|t| !t.title.trim().is_empty() && !artist_of(t).trim().is_empty());
        stream_place(named, self.locale())
    }

    /// The text of `track` can be kept in its artist's folder of the
    /// library: it has no file, and an artist and a title to name it by.
    pub(crate) fn lyrics_folder(&self, track: TrackId) -> bool {
        let local = self.lib.active_source(track).ok().flatten().is_some_and(|s| s.kind.is_local());
        let named = self.lib.get_track(track).ok().flatten().is_some_and(|t| !t.title.trim().is_empty() && !artist_of(&t).trim().is_empty());
        !local && named && self.import.is_some()
    }

    /// Where the text of `track` is kept while it has no file: its
    /// artist's folder of the library.
    pub(crate) fn kept_text(&self, track: TrackId) -> Option<PathBuf> {
        let dir = self.import.as_ref()?.config().library_dir.clone();
        let t = self.lib.get_track(track).ok().flatten()?;
        Some(nikgob_import::layout::kept_text_path(&dir, &artist_of(&t), &t.title))
    }

    /// `Command::SaveLyrics`: the shown text with its timing goes onto the
    /// computer (see the module docs).
    pub(crate) fn save_lyrics(&mut self, track: TrackId, source: LyricsSource) {
        let Some(tl) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)) else { return };
        let Some(keep) = tl.to_keep(source).map(Keep::to_millis) else { return };
        // The user's own text with the timing made of it stays theirs.
        if source == LyricsSource::Own {
            self.keep_own(track, keep);
            return;
        }
        let (place, folder) = (tl.place.clone(), tl.folder);
        match place {
            Place::File => {
                let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
                let file = self.res.path(&src.locator);
                self.keep_next_to(track, file, keep);
            }
            Place::Stream if self.state.read().prefs.lyrics_stream_keep == 1 => self.keep_with_song(track, keep),
            // Only the text: into the artist's folder (also for a song
            // whose timing cannot be made: a clip, another service).
            _ if folder => {
                let Some(path) = self.kept_text(track) else { return };
                let Some(svc) = self.import.as_ref() else { return };
                let tx = svc.internal();
                let loc = self.locale();
                svc.spawn_blocking(move || {
                    let result = write_ttml(&path, &keep, loc);
                    let _ = tx.send(Internal::Align(Msg::Kept { track, result }));
                });
            }
            _ => {}
        }
    }

    /// Where the user's own text of `track` is kept: next to its file, or
    /// for a song without one in its artist's folder of the library.
    pub(crate) fn own_path(&self, track: TrackId) -> Option<PathBuf> {
        if let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) {
            return Some(self.res.path(&src.locator).with_extension("ttml"));
        }
        if self.lyrics_folder(track) { self.kept_text(track) } else { None }
    }

    /// `Command::SaveOwnLyrics`: the text the user edited by hand is kept
    /// as theirs (see the module docs).
    pub(crate) fn save_own_lyrics(&mut self, track: TrackId, lines: Vec<LyricLine>, timed: bool) {
        self.keep_own(track, Keep::own(lines, timed).to_millis());
    }

    fn keep_own(&mut self, track: TrackId, keep: Keep) {
        let loc = self.locale();
        let Some(path) = self.own_path(track) else {
            // A song without a file and without an artist or a title to
            // keep its text under.
            self.toast_err(activity_in(loc).save_text_unnamed, None);
            return;
        };
        let Some(svc) = self.import.as_ref() else { return };
        let (trash, tx) = (svc.config().trash_dir.clone(), svc.internal());
        svc.spawn_blocking(move || {
            let result = keep_own_at(&path, &trash, &keep, loc);
            let _ = tx.send(Internal::Align(Msg::KeptOwn { track, result }));
        });
    }

    /// The user's own text is on the computer: it is shown from now on, as
    /// it is (whatever was picked or chosen to be seen before).
    pub(crate) fn own_lyrics_kept(&mut self, track: TrackId, result: Result<Text, String>) {
        let text = match result {
            Ok(t) => t,
            Err(e) => {
                log::warn!("saving the own lyrics: {e}");
                let text = format!("{}: {e}", activity_in(self.locale()).save_text_failed);
                self.toast_err(text, None);
                return;
            }
        };
        if let Err(e) = self.lib.set_lyrics_pick(track, None) {
            log::warn!("saving lyrics pick: {e}");
        }
        let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) else { return };
        tl.set_text(LyricsSource::Own, Some(text));
        tl.set_pick(None);
        if tl.display.take().is_some()
            && let Err(e) = self.lib.set_lyrics_show(track, None)
        {
            log::warn!("forgetting how lyrics are seen: {e}");
        }
        let view = tl.view(track);
        self.publish(|s| {
            if s.lyrics.track == Some(track) {
                s.lyrics = view;
            }
        });
    }

    /// `Command::DropOwnLyrics` («Вернуть найденный»): the user's own text
    /// goes to the library's Trash, with undo, and the found texts are
    /// shown again.
    pub(crate) fn drop_own_lyrics(&mut self, track: TrackId) {
        let Some(path) = self.own_path(track) else { return };
        let Some(trash) = self.import.as_ref().map(|s| s.config().trash_dir.clone()) else { return };
        let undo = match drop_own_at(&path, &trash) {
            Ok(Some(at)) => {
                self.align.own_trashed.insert(track, (at, path));
                true
            }
            Ok(None) => false,
            Err(e) => {
                log::warn!("removing the own lyrics of {track}: {e}");
                let text = format!("{}: {e}", activity_in(self.locale()).remove_own_failed);
                self.toast_err(text, None);
                return;
            }
        };
        if self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)).is_some_and(|tl| tl.picked() == Some(LyricsSource::Own))
            && let Err(e) = self.lib.set_lyrics_pick(track, None)
        {
            log::warn!("saving lyrics pick: {e}");
        }
        if let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) {
            tl.forget(LyricsSource::Own);
            let view = tl.view(track);
            self.publish(|s| {
                if s.lyrics.track == Some(track) {
                    s.lyrics = view;
                }
            });
        }
        if undo {
            let act = self.undo_action(nikgob_view::Command::RestoreOwnLyrics(track));
            self.toast_ok(activity_in(self.locale()).own_in_trash, Some(act));
        }
    }

    /// `Command::RestoreOwnLyrics`: the user's own text comes back from the
    /// Trash and is shown again.
    pub(crate) fn restore_own_lyrics(&mut self, track: TrackId) {
        let Some((at, to)) = self.align.own_trashed.remove(&track) else { return };
        let loc = self.locale();
        match restore_own_at(&at, &to, loc) {
            Ok(text) => self.own_lyrics_kept(track, Ok(text)),
            Err(e) => {
                log::warn!("bringing back the own lyrics of {track}: {e}");
                let text = format!("{}: {e}", activity_in(loc).restore_own_failed);
                self.toast_err(text, None);
            }
        }
    }

    /// The `.ttml` next to `file` at once, the file's tag once the file is
    /// not playing (rewriting it under the decoder shifts its pages).
    fn keep_next_to(&mut self, track: TrackId, file: PathBuf, keep: Keep) {
        let in_use = |e: Option<crate::queue::Entry>| e.is_some_and(|e| e.track == track);
        let later = in_use(self.queue.current()) || in_use(self.queue.peek_auto_next());
        if later {
            self.align.tag_later.insert(track, (file.clone(), keep.clone()));
        }
        let Some(svc) = self.import.as_ref() else { return };
        let tx = svc.internal();
        let loc = self.locale();
        svc.spawn_blocking(move || {
            let result = write_ttml(&file.with_extension("ttml"), &keep, loc).and_then(|text| {
                if !later {
                    write_tag(&file, &keep)?;
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
            let Some((file, keep)) = self.align.tag_later.remove(&t) else { continue };
            svc.spawn_blocking(move || {
                if let Err(e) = write_tag(&file, &keep) {
                    log::warn!("saving lyrics into {}: {e}", file.display());
                }
            });
        }
    }

    /// «Песню и текст»: the stream's audio is saved and imported as the
    /// track's file (as «Сохранить с YouTube» does), then the text goes
    /// next to it ([`Self::lyrics_imported`]).
    fn keep_with_song(&mut self, track: TrackId, keep: Keep) {
        let loc = self.locale();
        let Some(yt) = self.service(Service::Youtube) else {
            self.toast_err(activity_in(loc).yt_music_off, None);
            return;
        };
        let Some(src) = self.timing_stream(track).filter(|s| s.kind == SourceKind::Youtube) else { return };
        self.align.after_import.insert(track, keep);
        let dir = self.save_dir();
        let locator = src.locator.clone();
        self.streaming.spawn_internal(async move {
            let result = yt.save_audio(&locator, &dir).await.map_err(|e| e.to_string());
            Internal::Align(Msg::SongSaved { track, result })
        });
        self.toast_work(activity_in(loc).saving_song_yt, None);
    }

    pub(crate) fn song_saved(&mut self, track: TrackId, result: Result<PathBuf, String>) {
        let loc = self.locale();
        match result {
            Ok(path) => {
                self.align.saved_audio.insert(path.clone(), track);
                let targets = nikgob_import::ImportTargets { tracks: vec![track], force: true, ..Default::default() };
                if !self.import_for(
                    vec![path.clone()],
                    nikgob_core::i18n::status::StoredLine::SavingFromYoutube.encode(),
                    nikgob_core::model::SourceOrigin::Stream,
                    targets,
                ) {
                    self.align.saved_audio.remove(&path);
                    self.align.after_import.remove(&track);
                    let _ = std::fs::remove_file(&path);
                }
            }
            Err(e) => {
                log::warn!("saving a song for its lyrics: {e}");
                self.align.after_import.remove(&track);
                let text = format!("{}: {e}", activity_in(loc).save_song_failed);
                self.toast_err(text, None);
            }
        }
    }

    /// A file landed for `track` (or was found to be its file): a song
    /// saved for its text gets the text next to it, and the text it kept
    /// in its artist's folder goes; any other song that kept a text there
    /// while it had no file takes it along next to the file (unless one
    /// lies there already).
    pub(crate) fn lyrics_imported(&mut self, track: TrackId, path: &Path) {
        let asked = self.align.saved_audio.remove(path);
        if asked.is_some() {
            let _ = std::fs::remove_file(path);
        }
        let keep = asked.and_then(|t| self.align.after_import.remove(&t)).or_else(|| self.align.after_import.remove(&track));
        let Some(src) = self.lib.active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else { return };
        let file = self.res.path(&src.locator);
        let kept = self.kept_text(track).map(|k| nikgob_import::sanitize::long_path(&k)).filter(|k| k.is_file());
        let place = self.lyrics_place(track);
        if let Some(tl) = self.import.as_mut().and_then(|s| s.lyrics_cache.get_mut(&track)) {
            tl.place = place;
            tl.folder = false;
        }
        match (keep, kept) {
            (Some(keep), kept) => {
                if let Some(k) = kept
                    && let Err(e) = std::fs::remove_file(&k)
                {
                    log::warn!("removing {}: {e}", k.display());
                }
                self.keep_next_to(track, file, keep);
            }
            (None, Some(k)) => {
                let next_to = nikgob_import::sanitize::long_path(&file.with_extension("ttml"));
                if !next_to.exists()
                    && let Err(e) = std::fs::rename(&k, &next_to)
                {
                    log::warn!("moving {} next to its file: {e}", k.display());
                }
            }
            (None, None) => {}
        }
    }

    /// A saved song's file could not be imported: it goes, and so does the
    /// text that waited for it.
    pub(crate) fn lyrics_import_failed(&mut self, path: &Path) {
        let Some(track) = self.align.saved_audio.remove(path) else { return };
        let _ = std::fs::remove_file(path);
        if self.align.after_import.remove(&track).is_some() {
            self.toast_err(activity_in(self.locale()).save_song_failed, None);
        }
    }

    /// The text is on the computer: it is the track's own text now, shown
    /// as the player picks it (a file's timed text wins).
    pub(crate) fn lyrics_kept(&mut self, track: TrackId, result: Result<Text, String>) {
        let text = match result {
            Ok(t) => t,
            Err(e) => {
                log::warn!("saving lyrics: {e}");
                let text = format!("{}: {e}", activity_in(self.locale()).save_text_failed);
                self.toast_err(text, None);
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
    fn a_stream_is_timed_when_it_is_named() {
        assert_eq!(stream_place(true, Locale::Ru), Place::Stream, "a song, a cover, a named clip: what plays is timed");
        assert_eq!(stream_place(false, Locale::Ru), Place::Unclear(unnamed(Locale::Ru).into()), "no artist or title to keep it under");
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
        let text = write_ttml(&file.with_extension("ttml"), &Keep::Lines(lines.clone()), nikgob_core::i18n::Locale::Ru).unwrap();
        write_tag(&file, &Keep::Lines(lines.clone())).unwrap();
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
        write_ttml(&path, &Keep::Lines(lines.clone()), nikgob_core::i18n::Locale::Ru).unwrap();
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

    /// A runtime on a temp library in `dir` (nothing goes under the real
    /// one) with the fake YouTube, its aligner's parts marked as there (no
    /// runtime in them: a job fails at once instead of downloading).
    fn rig(lib: &nikgob_library::Library, prefs: nikgob_view::Prefs, dir: &Path) -> (crate::Runtime, crate::ImportConfig) {
        let cfg = crate::ImportConfig::with_root(dir);
        for d in [&cfg.library_dir, &cfg.inbox_dir, &cfg.downloads_dir, &cfg.trash_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let models = dir.join("models");
        for s in nikgob_align::catalog::ALL {
            std::fs::create_dir_all(models.join(s.dir)).unwrap();
            std::fs::write(models.join(s.dir).join(".ready"), b"").unwrap();
        }
        let yt = nikgob_core::streaming::FakeService::youtube().logged_out();
        let rt = crate::Runtime::start(
            crate::RuntimeConfig {
                library: lib.clone(),
                streams: vec![std::sync::Arc::new(yt)],
                library_dir: cfg.library_dir.clone(),
                prefs,
                prefs_path: None,
                settings_path: None,
                import: Some(crate::ImportSetup {
                    config: cfg.clone(),
                    metadata: None,
                    watch_inbox: false,
                    torrent_state_dir: dir.join("torrent-state"),
                    torrent_offline: true,
                    models_dir: Some(models),
                }),
            },
            Box::new(crate::NullPlayback),
            std::sync::Arc::new(|| {}),
        );
        (rt, cfg)
    }

    const PLAIN: &str = "Знаешь, не беда\nСтрока без слов";

    fn prefs() -> nikgob_view::Prefs {
        nikgob_view::Prefs { lyrics_gpu: false, lyrics_models_auto: false, ..Default::default() }
    }

    fn wait(rt: &crate::Runtime, what: &str, f: impl Fn(&nikgob_view::ViewState) -> bool) {
        let t = std::time::Instant::now();
        loop {
            let s = rt.state();
            if f(&s) {
                return;
            }
            assert!(
                t.elapsed() < Duration::from_secs(30),
                "timed out waiting for {what}: lyrics {:?}, craft {:?}, toasts {:?}",
                s.lyrics,
                s.align.craft,
                s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
            );
            drop(s);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let t = std::time::Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A fake YouTube track in the library with Genius' text, and (`made`)
    /// the word timing the player made of it.
    fn stream_track(lib: &nikgob_library::Library, id: &str, made: bool) -> TrackId {
        let t = nikgob_core::streaming::FakeService::youtube().tracks().into_iter().find(|t| t.id.id == id).unwrap();
        let track = lib.upsert_stream_track(&t).unwrap();
        let text = Text::from_plain(PLAIN).unwrap();
        lib.put_lyrics_text(track, LyricsSource::Genius.key(), text.body().as_deref()).unwrap();
        if made {
            let row = nikgob_library::AlignedLyrics {
                source: LyricsSource::Genius.key().to_owned(),
                text_hash: text.hash(),
                fit: 1.0,
                timing: "none".into(),
                offset_ms: 0,
                covered: 1.0,
                body: serde_json::to_string(&to_millis(&timed())).ok(),
                made_by: crate::align::MADE_BY.into(),
                made_at: 0,
                saved: true,
                fits: true,
            };
            lib.put_lyrics_aligned(track, &row).unwrap();
        }
        track
    }

    fn play(rt: &crate::Runtime, track: TrackId) {
        rt.send(nikgob_view::Command::PlayTracks { tracks: vec![track], start: 0, context: "lyrics test".into() });
        wait(rt, "the track's lyrics", |s| s.lyrics.track == Some(track) && s.lyrics.source.is_some());
    }

    /// The files under `dir`, relative, sorted.
    fn files(dir: &Path) -> Vec<String> {
        fn walk(d: &Path, root: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// The track's own file, once it has one.
    fn file_of(lib: &nikgob_library::Library, cfg: &crate::ImportConfig, track: TrackId) -> Option<PathBuf> {
        let src = lib.active_source(track).unwrap().filter(|s| s.kind.is_local())?;
        let p = Path::new(&src.locator);
        Some(if p.is_absolute() { p.to_owned() } else { cfg.library_dir.join(p) })
    }

    /// A song YouTube Music has as a record is timed from its stream: the
    /// audio is fetched into a temp folder and gone after the aligner
    /// tried; a named clip is offered the same; with the setting off
    /// nothing is offered.
    #[test]
    fn a_stream_is_timed_from_its_audio_fetched_for_a_while() {
        let dir = tempfile::tempdir().unwrap();
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let song = stream_track(&lib, "yt-gruppa-2", false);
        let clip = stream_track(&lib, "yt-video-kino", false);
        let (rt, cfg) = rig(&lib, prefs(), dir.path());
        play(&rt, song);
        wait(&rt, "timing offered", |s| s.lyrics.craft == nikgob_view::LyricsCraft::Ready && s.lyrics.stream);
        rt.send(nikgob_view::Command::CraftLyrics { track: song, source: LyricsSource::Genius, target: nikgob_view::LyricsSync::Word, deep: false });
        wait(&rt, "the aligner's answer", |s| s.align.craft.as_ref().is_some_and(|c| c.failed.is_some()));
        let failed = rt.state().align.craft.clone().unwrap().failed.unwrap();
        assert_ne!(failed, "Не удалось получить звук песни", "the audio came");
        let temp = cfg.downloads_dir.join(".align");
        assert!(temp.is_dir(), "fetched there");
        assert_eq!(files(&temp), Vec::<String>::new(), "and gone");
        assert_eq!(files(&cfg.library_dir), Vec::<String>::new(), "nothing kept");

        play(&rt, clip);
        // A clip that stays one is timed from its own audio; its text can
        // be kept in the artist's folder.
        wait(&rt, "a clip offered too", |s| s.lyrics.track == Some(clip) && s.lyrics.craft == nikgob_view::LyricsCraft::Ready && s.lyrics.stream && s.lyrics.savable);


        let mut off = rt.state().prefs.clone();
        off.lyrics_streams = false;
        rt.send(nikgob_view::Command::SetPrefs(Box::new(off)));
        play(&rt, song);
        wait(&rt, "nothing offered", |s| s.lyrics.track == Some(song) && s.lyrics.craft == nikgob_view::LyricsCraft::Off);
    }

    /// «Только текст»: the `.ttml` goes into the artist's folder of the
    /// (temp) library and is the song's text again in a new run; once the
    /// song gets a file (a YouTube copy here), the text moves next to it.
    #[test]
    fn a_stream_s_text_is_kept_in_its_artist_s_folder_and_found_again() {
        let dir = tempfile::tempdir().unwrap();
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let song = stream_track(&lib, "yt-gruppa-2", true);
        let row = lib.get_track(song).unwrap().unwrap();
        let kept = {
            let (rt, cfg) = rig(&lib, prefs(), dir.path());
            play(&rt, song);
            wait(&rt, "savable", |s| s.lyrics.savable && s.lyrics.stream);
            rt.send(nikgob_view::Command::SaveLyrics { track: song, source: LyricsSource::Genius });
            wait(&rt, "the kept text shown", |s| s.lyrics.source == Some(LyricsSource::File));
            assert_eq!(rt.state().lyrics.home, nikgob_view::LyricsHome::Computer);
            let kept = nikgob_import::layout::kept_text_path(&cfg.library_dir, &artist_of(&row), &row.title);
            assert!(kept.is_file(), "{}", kept.display());
            assert_eq!(files(&cfg.library_dir).len(), 1, "{:?}", files(&cfg.library_dir));
            assert!(file_of(&lib, &cfg, song).is_none(), "still a stream");
            kept
        };

        // A new run reads it back for the stream.
        let (rt, cfg) = rig(&lib, prefs(), dir.path());
        play(&rt, song);
        wait(&rt, "the kept text again", |s| {
            s.lyrics.source == Some(LyricsSource::File) && s.lyrics.lines.iter().any(|l| !l.words.is_empty())
        });

        // The song gets a file: the text goes next to it.
        rt.send(nikgob_view::Command::Like(nikgob_core::model::LikeTarget::Track(song), true));
        wait_until("the wish", || !lib.wishlist_for_track(song).unwrap().is_empty());
        let item = lib.wishlist_for_track(song).unwrap().pop().unwrap();
        rt.send(nikgob_view::Command::WishlistSaveFromYoutube(item.id));
        wait_until("the copy", || file_of(&lib, &cfg, song).is_some());
        let file = file_of(&lib, &cfg, song).unwrap();
        wait_until("the text next to it", || file.with_extension("ttml").is_file());
        assert!(!kept.exists(), "moved, not copied");
    }

    /// «Песню и текст»: the song is saved from YouTube and imported as the
    /// track's file (the YouTube copy), its text next to it, a text it
    /// kept in the artist's folder before goes, and the saved audio is
    /// not left behind.
    #[test]
    fn saving_a_stream_with_its_song_makes_it_local() {
        let dir = tempfile::tempdir().unwrap();
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let song = stream_track(&lib, "yt-gruppa-2", true);
        let row = lib.get_track(song).unwrap().unwrap();
        let (rt, cfg) = rig(&lib, nikgob_view::Prefs { lyrics_stream_keep: 1, ..prefs() }, dir.path());
        play(&rt, song);
        wait(&rt, "savable", |s| s.lyrics.savable && s.lyrics.stream);
        let kept = nikgob_import::layout::kept_text_path(&cfg.library_dir, &artist_of(&row), &row.title);
        write_ttml(&kept, &Keep::Lines(to_millis(&timed())), nikgob_core::i18n::Locale::Ru).unwrap();
        rt.send(nikgob_view::Command::SaveLyrics { track: song, source: LyricsSource::Genius });
        wait_until("the song's file", || file_of(&lib, &cfg, song).is_some());
        let src = lib.active_source(song).unwrap().unwrap();
        assert_eq!(src.quality_score, nikgob_core::model::score::SAVED_STREAM, "a YouTube copy");
        let file = file_of(&lib, &cfg, song).unwrap();
        assert!(file.starts_with(&cfg.library_dir), "{}", file.display());
        wait_until("the text next to it", || file.with_extension("ttml").is_file());
        wait(&rt, "its text shown", |s| s.lyrics.source == Some(LyricsSource::File) && !s.lyrics.stream);
        assert!(!kept.exists(), "the text kept before goes");
        wait_until("the saved audio gone", || files(&cfg.downloads_dir).is_empty());
    }

    /// The user's own text goes to the song's `.ttml` marked as theirs; a
    /// found text that lay there goes to the Trash first, and the player
    /// reads the own text as [`LyricsSource::Own`] (the file's text from
    /// what else it has). Going back puts it in the Trash too, and the undo
    /// brings it back unless something lies there now.
    #[test]
    fn an_own_text_keeps_what_lay_there_and_goes_to_the_trash_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join("Trash");
        let file = dir.path().join("Song.opus");
        let ttml = file.with_extension("ttml");
        std::fs::write(file.with_extension("lrc"), "[00:01.00]Найденная строка").unwrap();
        write_ttml(&ttml, &Keep::Plain("Найденный текст".into()), nikgob_core::i18n::Locale::Ru).unwrap();
        let s = Duration::from_secs;
        let own = Keep::own(vec![LyricLine::new(s(1), "Своя"), LyricLine::new(s(2), "строка")], true).to_millis();
        let text = keep_own_at(&ttml, &trash, &own, nikgob_core::i18n::Locale::Ru).unwrap();
        assert_eq!(text.lines.len(), 2);
        assert_eq!(files(&trash).len(), 1, "the found one kept in the Trash");
        assert!(crate::lyrics::read_ttml(&ttml).unwrap().1, "marked as the user's");
        // Saved again: the own text is rewritten, nothing more goes.
        keep_own_at(&ttml, &trash, &Keep::own(vec![LyricLine::new(s(1), "Своя"), LyricLine::new(s(2), "строка")], false), nikgob_core::i18n::Locale::Ru).unwrap();
        assert_eq!(files(&trash).len(), 1);
        assert_eq!(crate::lyrics::read_ttml(&ttml).unwrap().0.plain.as_deref(), Some("Своя\nстрока"));
        assert!(keep_own_at(&ttml, &trash, &Keep::own(vec![LyricLine::new(s(1), " ")], true), nikgob_core::i18n::Locale::Ru).is_err(), "an empty text is not kept");
        assert!(ttml.is_file());

        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let track = lib.upsert_track(&nikgob_core::model::Track::new("Song", nikgob_core::model::ArtistCredit::single("Artist", None))).unwrap().id;
        let tl = crate::lyrics::cached_lyrics(&lib, track, Some(&file), None, false, None);
        assert!(tl.texts_of(LyricsSource::Own).is_some());
        assert_eq!(tl.texts_of(LyricsSource::File).map(|t| t.lines[0].text.as_str()), Some("Найденная строка"));
        assert_eq!(tl.view(track).source, Some(LyricsSource::Own), "first over the file's timed text");

        let at = drop_own_at(&ttml, &trash).unwrap().unwrap();
        assert!(!ttml.exists());
        assert_eq!(files(&trash).len(), 2);
        assert_eq!(drop_own_at(&ttml, &trash).unwrap(), None, "nothing of the user's there");
        let tl = crate::lyrics::cached_lyrics(&lib, track, Some(&file), None, false, None);
        assert!(tl.texts_of(LyricsSource::Own).is_none());
        std::fs::write(&ttml, "x").unwrap();
        assert!(restore_own_at(&at, &ttml, nikgob_core::i18n::Locale::Ru).is_err(), "never over another file");
        std::fs::remove_file(&ttml).unwrap();
        assert_eq!(restore_own_at(&at, &ttml, nikgob_core::i18n::Locale::Ru).unwrap().plain.as_deref(), Some("Своя\nстрока"));
        assert!(crate::lyrics::read_ttml(&ttml).unwrap().1);
        assert_eq!(files(&trash).len(), 1);
    }

    /// A song without a file keeps the user's own text in its artist's
    /// folder of the (temp) library: shown first over the found text and
    /// again in a new run; «Вернуть найденный» puts it in the Trash and the
    /// found text is back, and the undo brings it back.
    #[test]
    fn a_stream_s_own_text_is_shown_first_kept_and_taken_back_with_undo() {
        let dir = tempfile::tempdir().unwrap();
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let song = stream_track(&lib, "yt-gruppa-2", false);
        let row = lib.get_track(song).unwrap().unwrap();
        let lines = vec![LyricLine::new(Duration::from_millis(1_500), "Знаешь, не беда"), LyricLine::new(Duration::from_millis(5_000), "Своя строка")];
        let own = |s: &nikgob_view::ViewState| s.lyrics.source == Some(LyricsSource::Own) && s.lyrics.lines.len() == 2;
        {
            let (rt, cfg) = rig(&lib, prefs(), dir.path());
            play(&rt, song);
            wait(&rt, "the found text", |s| s.lyrics.source == Some(LyricsSource::Genius));
            rt.send(nikgob_view::Command::SaveOwnLyrics { track: song, lines: lines.clone(), timed: true });
            wait(&rt, "the own text shown", own);
            let kept = nikgob_import::layout::kept_text_path(&cfg.library_dir, &artist_of(&row), &row.title);
            assert!(nikgob_metadata::ttml_is_own(&std::fs::read_to_string(&kept).unwrap()));
        }
        let (rt, cfg) = rig(&lib, prefs(), dir.path());
        play(&rt, song);
        wait(&rt, "the own text again", own);
        let kept = nikgob_import::layout::kept_text_path(&cfg.library_dir, &artist_of(&row), &row.title);
        rt.send(nikgob_view::Command::DropOwnLyrics(song));
        wait(&rt, "the found text again", |s| s.lyrics.source == Some(LyricsSource::Genius));
        assert!(!kept.exists());
        assert_eq!(files(&cfg.trash_dir).len(), 1);
        let undo = nikgob_view::Command::RestoreOwnLyrics(song);
        assert!(rt.state().toasts.iter().any(|t| t.action.as_ref().is_some_and(|(_, c)| *c == undo)));
        rt.send(undo);
        wait(&rt, "the own text back", own);
        assert!(kept.is_file());
        assert!(files(&cfg.trash_dir).is_empty());
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
