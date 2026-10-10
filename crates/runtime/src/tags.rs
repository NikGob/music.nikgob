//! The tag editor: read the file's tags, suggest MusicBrainz matches, write
//! the result back through the importer (which moves the file into place).

use std::time::Duration;

use nikgob_core::model::TrackId;
use nikgob_view as v;

use crate::Worker;
use crate::import::Internal;

fn tag_edit(t: &nikgob_import::tags::TagSet) -> v::TagEdit {
    let num = |n: Option<u32>| n.filter(|n| *n > 0).map(|n| n.to_string()).unwrap_or_default();
    v::TagEdit {
        title: t.title.clone().unwrap_or_default(),
        artist: t.artist.clone().unwrap_or_default(),
        album: t.album.clone().unwrap_or_default(),
        album_artist: t.album_artist.clone().unwrap_or_default(),
        year: t.year.map(|y| y.to_string()).unwrap_or_default(),
        track_no: num(t.track_no),
        disc_no: num(t.disc_no),
        genre: t.genre.clone().unwrap_or_default(),
        recording_mbid: t.recording_mbid.clone().unwrap_or_default(),
        release_mbid: t.release_mbid.clone().unwrap_or_default(),
    }
}

fn changes(e: v::TagEdit) -> nikgob_import::TagChanges {
    let num = |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n > 0);
    nikgob_import::TagChanges {
        year: Some(e.year.trim().get(..4).and_then(|y| y.parse().ok())),
        track_no: Some(num(&e.track_no)),
        disc_no: Some(num(&e.disc_no)),
        title: Some(e.title),
        artist: Some(e.artist),
        album: Some(e.album),
        album_artist: Some(e.album_artist),
        genre: Some(e.genre),
        recording_mbid: (!e.recording_mbid.is_empty()).then_some(e.recording_mbid),
        release_mbid: (!e.release_mbid.is_empty()).then_some(e.release_mbid),
    }
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// MusicBrainz recordings → editor suggestions, best first, no duplicates.
fn suggestions(recs: &[nikgob_metadata::musicbrainz::Recording], genre: &str) -> Vec<(String, v::TagEdit)> {
    let mut out: Vec<(String, v::TagEdit)> = Vec::new();
    for rec in recs {
        for c in nikgob_metadata::candidates_from_recording(rec) {
            let m = &c.metadata;
            let year = m.date.as_deref().and_then(|d| d.get(..4)).unwrap_or_default();
            let edit = v::TagEdit {
                title: m.title.clone(),
                artist: m.artist.clone(),
                album: m.album.clone().unwrap_or_default(),
                album_artist: m.album_artist.clone().unwrap_or_default(),
                year: year.to_owned(),
                track_no: m.track_no.map(|n| n.to_string()).unwrap_or_default(),
                disc_no: m.disc_no.map(|n| n.to_string()).unwrap_or_default(),
                genre: genre.to_owned(),
                recording_mbid: c.ids.recording_mbid.clone().unwrap_or_default(),
                release_mbid: c.ids.release_mbid.clone().unwrap_or_default(),
            };
            let mut label = format!("{} — {}", edit.title, edit.artist);
            if !edit.album.is_empty() {
                label.push_str(&format!(" · {}", edit.album));
            }
            if !year.is_empty() {
                label.push_str(&format!(" ({year})"));
            }
            if let Some(t) = &m.release_type {
                label.push_str(&format!(" · {t}"));
            }
            if !out.iter().any(|(l, _)| *l == label) {
                out.push((label, edit));
            }
            if out.len() >= 12 {
                return out;
            }
        }
    }
    out
}

impl Worker {
    pub(crate) fn open_tag_editor(&mut self, track: TrackId) {
        let Some(svc) = &self.import else { return };
        match svc.importer().read_track_tags(track) {
            Ok((_, t)) => {
                let editor = v::TagEditor {
                    track: Some(track),
                    current: tag_edit(&t),
                    ..Default::default()
                };
                self.publish(|s, _| s.tag_editor = editor);
            }
            Err(e) => {
                log::warn!("tags of {track}: {e}");
                self.toast("У трека нет локального файла", None);
            }
        }
    }

    pub(crate) fn lookup_tags(&mut self, track: TrackId, q: v::TagEdit) {
        let Some(svc) = &self.import else { return };
        let Some(meta) = svc.metadata() else {
            self.publish(|s, _| s.tag_editor.error = Some("Онлайн-поиск выключен".into()));
            return;
        };
        let duration = self
            .lib
            .get_track(track)
            .ok()
            .flatten()
            .and_then(|t| t.duration_ms)
            .map(|ms| Duration::from_millis(ms.into()));
        let tx = svc.internal();
        svc.spawn(async move {
            let mut query = nikgob_metadata::RecordingQuery::new(q.title.trim());
            query.artist = opt(&q.artist);
            query.release = opt(&q.album);
            query.duration = duration;
            let result = tokio::time::timeout(
                Duration::from_secs(20),
                meta.musicbrainz.search_recordings(&query),
            )
            .await
            .map_err(|_| "MusicBrainz не ответил".to_owned())
            .and_then(|r| r.map_err(|e| e.to_string()))
            .map(|recs| suggestions(&recs, &q.genre));
            let _ = tx.send(Internal::TagSuggestions(track, result));
        });
        self.publish(|s, _| {
            s.tag_editor.looking_up = true;
            s.tag_editor.error = None;
        });
    }

    pub(crate) fn save_tags(&mut self, track: TrackId, tags: v::TagEdit) {
        let Some(svc) = &self.import else { return };
        let importer = svc.importer().clone();
        let tx = svc.internal();
        svc.spawn_blocking(move || {
            let r = importer
                .retag(track, changes(tags))
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(Internal::Retagged(track, r));
        });
        self.publish(|s, _| s.tag_editor.loading = true);
    }

    pub(crate) fn tag_result(&mut self, msg: Internal) {
        match msg {
            Internal::TagSuggestions(track, r) => self.publish(|s, _| {
                if s.tag_editor.track != Some(track) {
                    return;
                }
                s.tag_editor.looking_up = false;
                match r {
                    Ok(list) if list.is_empty() => {
                        s.tag_editor.error = Some("Ничего не нашлось".into());
                    }
                    Ok(list) => s.tag_editor.suggestions = list,
                    Err(e) => s.tag_editor.error = Some(e),
                }
            }),
            Internal::Retagged(track, Ok(())) => {
                self.publish(|s, _| {
                    if s.tag_editor.track == Some(track) {
                        s.tag_editor = v::TagEditor::default();
                    }
                });
                self.toast("Теги сохранены", None);
                self.reload_after_import();
                if self.playing.map(|e| e.track) == Some(track) {
                    let current = self.lib.get_track_row(track).ok().flatten();
                    let src = self.lib.active_source(track).ok().flatten();
                    self.publish(|s, res| {
                        let row = current.as_ref().map(|r| res.track(r));
                        s.player.current = row.clone();
                        s.queue.current = row;
                        s.info = res.info(current.as_ref(), src.as_ref());
                    });
                    self.track_details(track);
                }
            }
            Internal::Retagged(_, Err(e)) => {
                log::warn!("retag: {e}");
                self.publish(|s, _| {
                    s.tag_editor.loading = false;
                    s.tag_editor.error = Some(format!("Не удалось сохранить: {e}"));
                });
            }
            _ => {}
        }
    }
}
