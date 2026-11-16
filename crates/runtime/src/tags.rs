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
        track_total: None,
        disc_no: Some(num(&e.disc_no)),
        title: Some(e.title),
        artist: Some(e.artist),
        album: Some(e.album),
        album_artist: Some(e.album_artist),
        genre: Some(e.genre),
        recording_mbid: (!e.recording_mbid.is_empty()).then_some(e.recording_mbid),
        release_mbid: (!e.release_mbid.is_empty()).then_some(e.release_mbid),
        release: None,
        keep_album_artist: false,
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
                self.publish(|s| s.tag_editor = editor);
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
            self.publish(|s| s.tag_editor.error = Some("Онлайн-поиск выключен".into()));
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
        self.publish(|s| {
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
        self.publish(|s| s.tag_editor.loading = true);
    }

    pub(crate) fn tag_result(&mut self, msg: Internal) {
        match msg {
            Internal::TagSuggestions(track, r) => self.publish(|s| {
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
                self.publish(|s| {
                    if s.tag_editor.track == Some(track) {
                        s.tag_editor = v::TagEditor::default();
                    }
                });
                self.toast("Теги сохранены", None);
                self.reload_after_import();
                self.retagged_playing(&[track]);
            }
            Internal::Retagged(_, Err(e)) => {
                log::warn!("retag: {e}");
                self.publish(|s| {
                    s.tag_editor.loading = false;
                    s.tag_editor.error = Some(format!("Не удалось сохранить: {e}"));
                });
            }
            _ => {}
        }
    }

    /// The song playing, if it is one of `tracks` that took new tags: its
    /// row and info again.
    fn retagged_playing(&mut self, tracks: &[TrackId]) {
        let Some(track) = self.playing.map(|e| e.track).filter(|t| tracks.contains(t)) else { return };
        // The view shows another device's zone: only the parked view.
        if self.connect.remote() {
            self.publish_parked();
            return;
        }
        let current = self.lib.get_track_row(track).ok().flatten();
        let src = self.lib.active_source(track).ok().flatten();
        let row = current.as_ref().map(|r| self.res.track(r));
        let info = self.res.info(current.as_ref(), src.as_ref());
        self.publish(|s| {
            s.player.current = row.clone();
            s.queue.current = row;
            s.info = info;
        });
        self.track_details(track);
    }

    /// The tags in the files of `tracks`, read off the loop, to `reply` as
    /// `{"tags": {"<id>": {"title", …}}, "outside": ["<id>"]}`: a track
    /// without a file is left out, one whose file lies outside the library
    /// is listed in `outside` and never read.
    pub(crate) fn api_read_tags(&mut self, tracks: Vec<TrackId>, reply: v::ApiReply) {
        let Some(svc) = &self.import else {
            reply.send(Err("Файлы библиотеки недоступны".to_owned()));
            return;
        };
        let importer = svc.importer().clone();
        svc.spawn_blocking(move || {
            let mut out = serde_json::Map::new();
            let mut outside = Vec::new();
            for t in tracks {
                if outside_library(&importer, t) {
                    outside.push(t.to_string());
                    continue;
                }
                match importer.read_track_tags(t) {
                    Ok((_, tags)) => {
                        out.insert(t.to_string(), edit_json(&tag_edit(&tags)));
                    }
                    Err(e) => log::debug!("api: tags of {t}: {e}"),
                }
            }
            reply.send(Ok(serde_json::json!({"tags": out, "outside": outside})));
        });
    }

    /// The local API's retag: each track's file read, checked against the
    /// values the confirmed plan showed (a field changed since is a
    /// failure, nothing written), the patch put over what it has and
    /// written (the importer moves the file into place, and refuses a file
    /// outside the library); one snackbar for all, to `reply` as
    /// `{"done", "failed"}`.
    pub(crate) fn api_retag(&mut self, tracks: Vec<(TrackId, v::TagPatch, v::TagPatch)>, reply: v::ApiReply) {
        let Some(svc) = &self.import else {
            reply.send(Err("Файлы библиотеки недоступны".to_owned()));
            return;
        };
        let importer = svc.importer().clone();
        let tx = svc.internal();
        svc.spawn_blocking(move || {
            let mut done = Vec::new();
            let mut failed = Vec::new();
            for (t, patch, before) in tracks {
                let r = importer.read_track_tags(t).map_err(|e| e.to_string()).and_then(|(_, now)| {
                    let now = tag_edit(&now);
                    if !still_as_planned(&now, &before) {
                        return Err("the tags changed since the plan".to_owned());
                    }
                    importer.retag(t, changes(patch.apply(&now))).map(|_| ()).map_err(|e| e.to_string())
                });
                match r {
                    Ok(()) => done.push(t),
                    Err(e) => {
                        log::warn!("api: retag of {t}: {e}");
                        failed.push(serde_json::json!({"id": t.to_string(), "error": e}));
                    }
                }
            }
            let failures = failed.len();
            reply.send(Ok(serde_json::json!({
                "done": done.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "failed": failed,
            })));
            let _ = tx.send(Internal::ApiRetagged { failed: failures, done });
        });
    }

    /// The API's retag came back: one snackbar, the library read again.
    pub(crate) fn api_retagged(&mut self, done: &[TrackId], failed: usize) {
        match (done.len(), failed) {
            (0, 0) => return,
            (0, _) => self.toast("Не удалось сохранить теги", None),
            (1, 0) => self.toast("Теги сохранены", None),
            (n, 0) => self.toast(format!("Теги сохранены: {n}"), None),
            (n, f) => self.toast(format!("Теги сохранены: {n}, не удалось: {f}"), None),
        }
        self.reload_after_import();
        self.retagged_playing(done);
    }
}

/// A [`v::TagEdit`] as the API answers it.
fn edit_json(e: &v::TagEdit) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for (k, val) in v::tag_fields(e) {
        m.insert(k.to_owned(), serde_json::Value::String(val.to_owned()));
    }
    serde_json::Value::Object(m)
}

/// Whether the track's file is there but lies outside the library and the
/// masters folder (links followed): the API never reads or writes it.
fn outside_library(importer: &nikgob_import::Importer, track: TrackId) -> bool {
    let Some(src) = importer.library().active_source(track).ok().flatten().filter(|s| s.kind.is_local()) else {
        return false;
    };
    let path = std::path::Path::new(&src.locator);
    path.exists() && importer.check_in_library(path).is_err()
}

/// Whether each field the plan showed still has the value it showed.
fn still_as_planned(now: &v::TagEdit, before: &v::TagPatch) -> bool {
    let now: std::collections::HashMap<&str, &str> = v::tag_fields(now).into_iter().collect();
    before.fields().into_iter().all(|(k, want)| want.is_none_or(|w| now.get(k).copied().unwrap_or_default().trim() == w.trim()))
}
