//! The stream part of the unified search: every chosen service is asked at
//! once and its section is published as it answers; a newer query aborts
//! the older requests and drops whatever they still bring. Rows are merged
//! against the library (a library track wins) and across services (the
//! first service, Spotify, keeps the row).

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{Service, StreamError, StreamSearch, StreamTrack};
use nikgob_metadata::text::{TitleKey, album_key, artist_key, similarity, title_key};
use nikgob_view as v;

use super::Msg;
use crate::Worker;

/// Wait before asking the services, so typing does not fire a request per
/// key (an older query's task is aborted while it waits).
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
/// How far two lengths may be apart for the same song.
const SLACK_MS: u64 = 3_000;
/// Artist names this similar (0..=1) are the same artist.
const ARTIST_SIMILARITY: f64 = 0.85;

#[derive(Default)]
pub(crate) struct SearchState {
    pub(crate) seq: u64,
    pub(crate) query: String,
    pub(crate) sources: v::SearchSources,
    /// Per asked service, in section order: `None` while it is searching.
    raw: Vec<(Service, Option<Result<StreamSearch, StreamError>>)>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Library hits of the query (they win over stream rows).
    lib_tracks: Vec<(nikgob_library::TrackRow, Option<String>)>,
}

/// What makes two tracks the same song: the ISRC, else artist + title
/// (folded like the importer folds them) + length within [`SLACK_MS`].
#[derive(Clone, Debug)]
pub(crate) struct SongKey {
    isrc: Option<String>,
    title: TitleKey,
    artist: String,
    duration_ms: Option<u64>,
}

fn upper_isrc(isrc: Option<&str>) -> Option<String> {
    let s: String = isrc?
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (!s.is_empty()).then_some(s)
}

/// The main artist of a credit line ("A, B", "A feat. B" → "a").
fn main_artist(line: &str) -> String {
    let first = line.split(", ").next().unwrap_or(line);
    artist_key(first)
}

impl SongKey {
    pub(crate) fn of_stream(t: &StreamTrack) -> Self {
        Self {
            isrc: upper_isrc(t.isrc.as_deref()),
            title: title_key(&t.title),
            artist: main_artist(
                t.artists
                    .first()
                    .map_or(t.artist_line().as_str(), |a| a.name.as_str()),
            ),
            duration_ms: t.duration_ms,
        }
    }

    pub(crate) fn of_library(row: &nikgob_library::TrackRow, isrc: Option<&str>) -> Self {
        Self {
            isrc: upper_isrc(isrc),
            title: title_key(&row.title),
            artist: main_artist(&row.artist),
            duration_ms: row.duration_ms.map(u64::from),
        }
    }

    pub(crate) fn same(&self, o: &Self) -> bool {
        if let (Some(a), Some(b)) = (&self.isrc, &o.isrc)
            && a == b
        {
            return true;
        }
        if self.title.base.is_empty() || self.title != o.title {
            return false;
        }
        let artist = self.artist == o.artist
            || (!self.artist.is_empty()
                && !o.artist.is_empty()
                && similarity(&self.artist, &o.artist) >= ARTIST_SIMILARITY);
        let length = match (self.duration_ms, o.duration_ms) {
            (Some(a), Some(b)) => a.abs_diff(b) <= SLACK_MS,
            _ => true,
        };
        artist && length
    }
}

impl Worker {
    /// The stream part of [`Command::Search`](nikgob_view::Command::Search):
    /// `library` are the library hits already published.
    pub(crate) fn search_streams(&mut self, query: &str, library: Vec<nikgob_library::TrackRow>) {
        let st = &mut self.streaming.search;
        st.seq += 1;
        for t in st.tasks.drain(..) {
            t.abort();
        }
        st.query = query.to_owned();
        st.raw.clear();
        st.lib_tracks = library
            .into_iter()
            .map(|r| {
                let isrc = self.lib.get_track(r.id).ok().flatten().and_then(|t| t.isrc);
                (r, isrc)
            })
            .collect();
        let seq = st.seq;
        let sources = st.sources;
        if query.is_empty() {
            self.publish(|s, _| s.search.streams.clear());
            return;
        }
        for &service in Service::ALL {
            if !sources.service(service) {
                continue;
            }
            let Some(svc) = self.service(service) else { continue };
            let q = query.to_owned();
            let task = self.streaming.spawn(async move {
                tokio::time::sleep(DEBOUNCE).await;
                let result = svc.search(&q).await;
                Msg::Searched {
                    seq,
                    service,
                    result,
                }
            });
            let st = &mut self.streaming.search;
            st.tasks.push(task);
            st.raw.push((service, None));
        }
        self.rebuild_sections();
    }

    pub(crate) fn set_search_sources(&mut self, sources: v::SearchSources) {
        self.streaming.search.sources = sources;
        let q = self.state.read().search.query.clone();
        self.publish(|s, _| s.search.sources = sources);
        if !q.trim().is_empty() {
            self.search(q);
        }
    }

    pub(crate) fn stream_searched(
        &mut self,
        seq: u64,
        service: Service,
        result: Result<StreamSearch, StreamError>,
    ) {
        if seq != self.streaming.search.seq {
            return; // an older query
        }
        if let Err(e) = &result {
            log::warn!("{service} search: {e}");
        }
        if let Ok(r) = &result {
            self.streaming.learn(&r.tracks);
        }
        if let Some(slot) = self
            .streaming
            .search
            .raw
            .iter_mut()
            .find(|(s, _)| *s == service)
        {
            slot.1 = Some(result);
        }
        self.rebuild_sections();
    }

    /// Recomputes the stream sections from the raw answers (merging rows).
    pub(crate) fn rebuild_sections(&mut self) {
        let raw: Vec<(Service, Option<Result<StreamSearch, StreamError>>)> = self
            .streaming
            .search
            .raw
            .iter()
            .map(|(s, r)| (*s, r.clone()))
            .collect();
        let lib_keys: Vec<(TrackId, SongKey)> = self
            .streaming
            .search
            .lib_tracks
            .iter()
            .map(|(r, isrc)| (r.id, SongKey::of_library(r, isrc.as_deref())))
            .collect();
        let mut extra_lib: Vec<nikgob_library::TrackRow> = Vec::new();
        let mut kept: Vec<(SongKey, usize, usize)> = Vec::new();
        let mut kept_albums: Vec<(String, String)> = Vec::new();
        let mut kept_artists: Vec<String> = Vec::new();
        let mut sections: Vec<v::StreamSearchSection> = Vec::new();
        for (service, result) in raw {
            let mut section = v::StreamSearchSection::new(service);
            match result {
                None => section.searching = true,
                Some(Err(e)) => section.error = Some(super::error_text(&e)),
                Some(Ok(r)) => {
                    for t in &r.tracks {
                        let key = SongKey::of_stream(t);
                        if lib_keys.iter().any(|(_, k)| k.same(&key)) {
                            continue; // shown as the library hit
                        }
                        let linked = self.library_track_of(t);
                        if let Some(id) = linked
                            && self.lib.track_in_library(id).ok().flatten() == Some(true)
                        {
                            if !lib_keys.iter().any(|(t, _)| *t == id)
                                && !extra_lib.iter().any(|r| r.id == id)
                                && let Some(row) = self.lib.get_track_row(id).ok().flatten()
                            {
                                extra_lib.push(row);
                            }
                            continue;
                        }
                        if let Some((_, si, ri)) = kept.iter().find(|(k, ..)| k.same(&key)) {
                            let row: &mut v::StreamTrackRow = if *si == sections.len() {
                                &mut section.tracks[*ri]
                            } else {
                                &mut sections[*si].tracks[*ri]
                            };
                            if !row.also_on.contains(&service) && row.remote.service != service {
                                row.also_on.push(service);
                            }
                            continue;
                        }
                        kept.push((key, sections.len(), section.tracks.len()));
                        section.tracks.push(self.stream_row(t, linked));
                    }
                    for a in &r.albums {
                        let key = (
                            album_key(&a.title),
                            main_artist(&nikgob_core::streaming::join_names(&a.artists)),
                        );
                        if kept_albums.contains(&key) {
                            continue;
                        }
                        kept_albums.push(key);
                        section.albums.push(self.album_card(a));
                    }
                    for a in &r.artists {
                        let key = artist_key(&a.name);
                        if kept_artists.contains(&key) {
                            continue;
                        }
                        kept_artists.push(key);
                        section.artists.push(self.artist_card(a));
                    }
                    for p in &r.playlists {
                        section.playlists.push(self.playlist_card(p));
                    }
                }
            }
            sections.push(section);
        }
        let query = self.streaming.search.query.clone();
        self.publish(|s, res| {
            if s.search.query.trim() != query {
                return;
            }
            for row in &extra_lib {
                if !s.search.tracks.iter().any(|t| t.id == row.id) {
                    s.search.tracks.push(res.track(row));
                }
            }
            s.search.streams = sections;
        });
    }

    /// The library track a stream track is: linked by its remote id, else
    /// the one with its ISRC.
    pub(crate) fn library_track_of(&self, t: &StreamTrack) -> Option<TrackId> {
        if let Some(id) = self.lib.track_by_remote(&t.id).ok().flatten() {
            return Some(id);
        }
        let isrc = upper_isrc(t.isrc.as_deref())?;
        self.lib
            .tracks_by_isrc(&isrc)
            .ok()?
            .into_iter()
            .find(|t| self.lib.track_deleted_at(t.id).ok().flatten().is_none())
            .map(|t| t.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{ArtistRef, RemoteId};

    fn track(service: Service, title: &str, artist: &str, secs: u64) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(service, title), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = Some(secs * 1000);
        t
    }

    #[test]
    fn same_song_by_isrc_or_by_name_and_length() {
        let a = track(Service::Spotify, "Группа крови", "Кино", 285);
        let mut b = track(Service::Youtube, "Gruppa Krovi (Remastered 2019)", "Kino", 287);
        assert!(SongKey::of_stream(&a).same(&SongKey::of_stream(&b)));
        b.duration_ms = Some(295_000);
        assert!(!SongKey::of_stream(&a).same(&SongKey::of_stream(&b)), "10 s apart");
        b.isrc = Some("zz-fak-88-00001".into());
        let mut a2 = a.clone();
        a2.isrc = Some("ZZFAK8800001".into());
        assert!(SongKey::of_stream(&a2).same(&SongKey::of_stream(&b)), "same ISRC");
        let live = track(Service::Youtube, "Группа крови (Live)", "Кино", 285);
        assert!(!SongKey::of_stream(&a).same(&SongKey::of_stream(&live)));
        let other = track(Service::Youtube, "Группа крови", "Ария", 285);
        assert!(!SongKey::of_stream(&a).same(&SongKey::of_stream(&other)));
    }
}
