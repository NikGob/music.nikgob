//! Library rows → view rows: resolves cover paths, palette seeds and the
//! quality label. Caches per-release cover lookups.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::model::{ArtistId, Codec, PlaylistId, Release, ReleaseId, Source, SourceKind};
use nikgob_library::{
    ArtistRow, Library, PlaylistKind, PlaylistSummary, ReleaseProgress, ReleaseRow, TrackRow,
};
use nikgob_view as v;

use crate::home::Home;
use crate::playlist_cover;

/// A stream artist's image url (from a service) as a cover.
fn web_image(images: &[String]) -> Option<v::Cover> {
    images.iter().find(|i| nikgob_library::is_web_ref(i)).map(|i| v::Cover::web(i.clone()))
}

pub struct Resolver {
    lib: Library,
    root: PathBuf,
    covers: HashMap<ReleaseId, Option<v::Cover>>,
    /// Artist photos on disk; loaded on first use.
    photos: Option<HashMap<ArtistId, v::Cover>>,
}

impl Resolver {
    pub fn new(lib: Library, root: PathBuf) -> Self {
        Self {
            lib,
            root,
            covers: HashMap::new(),
            photos: None,
        }
    }

    /// The library root relative locators are under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A locator or cover ref as an absolute path.
    pub fn path(&self, locator: &str) -> PathBuf {
        let p = Path::new(locator);
        if p.is_absolute() {
            p.to_owned()
        } else {
            self.root.join(p)
        }
    }

    fn cover_for(&mut self, release: Option<ReleaseId>, cover_ref: Option<&str>) -> Option<v::Cover> {
        let id = release?;
        if let Some(c) = self.covers.get(&id) {
            return c.clone();
        }
        let cover_ref = cover_ref.map(str::to_owned).or_else(|| {
            self.lib
                .get_release(id)
                .ok()
                .flatten()
                .and_then(|r| r.cover_ref)
        });
        let cover = cover_ref.map(|r| {
            // A stream album's cover until a file brings one.
            if nikgob_library::is_web_ref(&r) {
                return v::Cover::web(r);
            }
            let path = self.path(&r);
            let ambient = path.with_file_name("ambient.png");
            let seed = self
                .lib
                .get_palette(id)
                .ok()
                .flatten()
                .map(|p| p.seed_argb);
            v::Cover {
                ambient: ambient.exists().then_some(ambient),
                path,
                seed,
            }
        });
        self.covers.insert(id, cover.clone());
        cover
    }

    /// The photo of an artist, if one was downloaded and is still on disk.
    pub fn photo(&mut self, id: ArtistId) -> Option<v::Cover> {
        if self.photos.is_none() {
            let stored = self.lib.artist_photos().unwrap_or_default();
            let map = stored
                .into_iter()
                .map(|(id, p)| (id, self.path(&p)))
                .filter(|(_, p)| p.exists())
                .map(|(id, path)| (id, v::Cover { path, seed: None, ambient: None }))
                .collect();
            self.photos = Some(map);
        }
        self.photos.as_ref().and_then(|m| m.get(&id)).cloned()
    }

    /// Reload artist photos from the library on next use.
    pub fn invalidate_photos(&mut self) {
        self.photos = None;
    }

    /// Forget cached covers (after an import changed them).
    pub fn invalidate(&mut self) {
        self.covers.clear();
    }

    /// The cover of a release (its file, or a service's https url).
    pub fn release_cover(&mut self, id: ReleaseId) -> Option<v::Cover> {
        self.cover_for(Some(id), None)
    }

    pub fn track(&mut self, r: &TrackRow) -> v::TrackRow {
        v::TrackRow {
            id: r.id,
            // A clip without its song by the song's title («AIYOU»).
            title: crate::streams::shown_title(r),
            artist: r.artist.clone(),
            album: r.album.clone(),
            release_id: r.release_id,
            artists: (&r.credit).into(),
            disc_no: r.disc_no,
            track_no: r.track_no,
            duration: r.duration_ms.map(|ms| Duration::from_millis(u64::from(ms))),
            quality: r.codec.as_ref().map(codec_label),
            source: r.active_kind,
            has_local: r.has_local,
            liked: r.liked,
            // A clip has no album: the video's own picture.
            cover: self.cover_for(r.release_id, r.cover_ref.as_deref()).or_else(|| {
                let video = r.youtube_id.as_deref().filter(|_| r.is_clip())?;
                Some(v::Cover::web(format!("https://i.ytimg.com/vi/{video}/hqdefault.jpg")))
            }),
            clip: r.is_clip(),
        }
    }

    pub fn release_card(&mut self, r: &Release, local: u32, total: Option<u32>, liked: bool) -> v::ReleaseCard {
        v::ReleaseCard {
            id: r.id,
            title: r.title.clone(),
            artist: r.artist_credit.display(),
            artists: (&r.artist_credit).into(),
            year: r.year,
            release_type: r.release_type,
            cover: self.cover_for(Some(r.id), r.cover_ref.as_deref()),
            local_tracks: local,
            total_tracks: total,
            liked,
        }
    }

    /// A streamed album outside the library as a card of its service.
    pub fn stream_album(
        &mut self,
        remote: &nikgob_core::streaming::RemoteId,
        r: &Release,
        progress: Option<ReleaseProgress>,
    ) -> v::StreamAlbumCard {
        let have = progress.map_or_else(v::AlbumHave::default, |p| v::AlbumHave {
            local: p.local,
            in_library: p.in_library,
            total: p.total.max(r.total_tracks.unwrap_or(0)),
        });
        v::StreamAlbumCard {
            remote: remote.clone(),
            title: r.title.clone(),
            artist: r.artist_credit.display(),
            // Library ids, not the service's: no links.
            artists: v::StreamArtistLinks::default(),
            year: r.year,
            release_type: r.release_type,
            cover_url: self
                .cover_for(Some(r.id), r.cover_ref.as_deref())
                .and_then(|c| c.web_url().map(str::to_owned)),
            total_tracks: r.total_tracks,
            release: None,
            have,
        }
    }

    pub fn release(&mut self, r: &ReleaseRow) -> v::ReleaseCard {
        let p = r.progress();
        self.release_card(&r.release, p.local, Some(p.total), r.liked)
    }

    pub fn artist(&mut self, a: &ArtistRow) -> v::ArtistCard {
        v::ArtistCard {
            id: a.artist.id,
            name: a.artist.name.clone(),
            image: self.photo(a.artist.id).or_else(|| web_image(&a.artist.images)),
            liked: self.lib.is_liked(a.artist.id).unwrap_or(false),
        }
    }

    pub fn playlist(&mut self, p: &PlaylistSummary) -> v::PlaylistCard {
        let cover_mode = playlist_cover::mode_of(p.cover.as_deref());
        let cover = match (p.id, cover_mode) {
            // "Любимые" keeps its heart.
            (None, _) => None,
            (Some(_), v::PlaylistCoverMode::Custom) => p.cover.as_ref().map(|c| v::Cover {
                path: self.path(c),
                seed: None,
                ambient: None,
            }),
            (Some(id), mode) => self.auto_cover(id, mode),
        };
        let service = p
            .id
            .and_then(|id| self.lib.playlist_remote(id).ok().flatten())
            .map(|link| link.remote.service);
        v::PlaylistCard {
            id: p.id.unwrap_or(v::LIKED_PLAYLIST),
            name: p.name.clone(),
            cover,
            track_count: p.track_count,
            pinned: p.kind == PlaylistKind::Liked,
            cover_mode,
            service,
        }
    }

    /// A collage / smart cover from the playlist's album covers.
    fn auto_cover(&mut self, id: PlaylistId, mode: v::PlaylistCoverMode) -> Option<v::Cover> {
        // Covers whose file is gone count as none (checked once per album).
        let mut exists: HashMap<ReleaseId, bool> = HashMap::new();
        let refs: Vec<(ReleaseId, PathBuf)> = self
            .lib
            .playlist_cover_refs(id)
            .unwrap_or_default()
            .into_iter()
            .map(|(r, c)| (r, self.path(&c)))
            .filter(|(r, p)| *exists.entry(*r).or_insert_with(|| p.exists()))
            .collect();
        let plain = |path: PathBuf| v::Cover { path, seed: None, ambient: None };
        match playlist_cover::pick(&refs, mode) {
            playlist_cover::Pick::Single(r) => self.cover_for(Some(r), None),
            playlist_cover::Pick::Collage(files) => playlist_cover::collage(&self.root, &files).map(plain),
            playlist_cover::Pick::Placeholder => {
                playlist_cover::placeholder(&self.root, &id.to_string()).map(plain)
            }
        }
    }

    pub fn release_detail(
        &mut self,
        r: &Release,
        rows: &[TrackRow],
        progress: Option<ReleaseProgress>,
        liked: bool,
    ) -> v::ReleaseDetail {
        let (local, total) = progress.map_or((0, None), |p| (p.local, Some(p.total)));
        let tracks: Vec<v::TrackRow> = rows.iter().map(|t| self.track(t)).collect();
        let total_duration = tracks.iter().filter_map(|t| t.duration).sum();
        let quality = summarize_quality(&tracks);
        v::ReleaseDetail {
            card: self.release_card(r, local, total, liked),
            tracks,
            total_duration,
            quality,
            liked,
        }
    }

    pub fn artist_detail(
        &mut self,
        a: &nikgob_core::model::Artist,
        top: &[TrackRow],
        releases: &[ReleaseRow],
        liked: bool,
    ) -> v::ArtistDetail {
        use nikgob_core::model::ReleaseType as RT;
        let mut albums = Vec::new();
        let mut singles = Vec::new();
        for r in releases {
            let card = self.release(r);
            match r.release.release_type {
                RT::Single | RT::Ep => singles.push(card),
                _ => albums.push(card),
            }
        }
        v::ArtistDetail {
            card: v::ArtistCard {
                id: a.id,
                name: a.name.clone(),
                image: self.photo(a.id).or_else(|| web_image(&a.images)),
                liked,
            },
            top_tracks: top.iter().map(|t| self.track(t)).collect(),
            albums,
            singles_eps: singles,
            liked,
            discography: v::DiscographyView::default(),
        }
    }

    pub fn playlist_detail(&mut self, p: &PlaylistSummary, rows: &[TrackRow]) -> v::PlaylistDetail {
        let tracks: Vec<v::TrackRow> = rows.iter().map(|t| self.track(t)).collect();
        let total_duration = tracks.iter().filter_map(|t| t.duration).sum();
        let mut card = self.playlist(p);
        if card.cover.is_none() {
            card.cover = tracks.iter().find_map(|t| t.cover.clone());
        }
        v::PlaylistDetail {
            card,
            tracks,
            total_duration,
            sync: None,
        }
    }

    pub fn info(&mut self, row: Option<&TrackRow>, src: Option<&Source>) -> v::TrackInfo {
        let Some(row) = row else {
            return v::TrackInfo::default();
        };
        let mut tags = vec![("Название".to_owned(), row.title.clone()), ("Исполнитель".to_owned(), row.artist.clone())];
        if let Some(a) = &row.album {
            tags.push(("Альбом".into(), a.clone()));
        }
        if row.track_no > 0 {
            tags.push(("Номер".into(), format!("{}-{}", row.disc_no, row.track_no)));
        }
        let quality = src.map(|s| {
            let mut q = codec_label(&s.codec);
            if let Some(b) = s.bitrate {
                q.push_str(&format!(" {b}"));
            }
            if let (Some(sr), Some(bd)) = (s.sample_rate, s.bit_depth) {
                // 24/44.1, 24/48.
                let khz = if sr % 1000 == 0 {
                    (sr / 1000).to_string()
                } else {
                    format!("{:.1}", f64::from(sr) / 1000.0)
                };
                q.push_str(&format!(" · {bd}/{khz}"));
            }
            q
        });
        v::TrackInfo {
            track: Some(row.id),
            quality,
            source: src.map(|s| {
                match s.kind {
                    SourceKind::LocalFile => "Локальный файл",
                    SourceKind::Spotify => "Spotify",
                    SourceKind::Youtube => "YouTube Music",
                    // Never plays itself (the YouTube copy does).
                    SourceKind::Deezer => "Deezer",
                }
                .to_owned()
            }),
            path: src
                .filter(|s| s.kind == SourceKind::LocalFile)
                .map(|s| self.path(&s.locator)),
            tags,
        }
    }

    pub fn home(&mut self, h: &Home) -> v::HomeView {
        v::HomeView {
            greeting_hour: h.hour,
            mix: h.mix.iter().map(|t| self.track(t)).collect(),
            continue_listening: h
                .continue_listening
                .iter()
                .map(|c| match c {
                    crate::home::Continue::Release(r) => v::ContinueCard::Release(self.release(r)),
                    crate::home::Continue::Stream { remote, release, progress } => {
                        v::ContinueCard::Stream(self.stream_album(remote, release, *progress))
                    }
                })
                .collect(),
            artist_mixes: h.artists.iter().map(|a| self.artist(a)).collect(),
            long_unplayed: h.long_unplayed.iter().map(|r| self.release(r)).collect(),
            recently_added: h.recently_added.iter().map(|r| self.release(r)).collect(),
            // The recommendations' parts come from `recs`.
            ..v::HomeView::default()
        }
    }
}

pub fn codec_label(c: &Codec) -> String {
    match c {
        Codec::Flac => "FLAC".into(),
        Codec::Opus => "Opus".into(),
        Codec::Mp3 => "MP3".into(),
        Codec::Aac => "AAC".into(),
        Codec::Vorbis => "Vorbis".into(),
        Codec::Alac => "ALAC".into(),
        Codec::Wav => "WAV".into(),
        Codec::Other(s) => s.to_uppercase(),
    }
}

fn summarize_quality(tracks: &[v::TrackRow]) -> Option<String> {
    let mut labels: Vec<&str> = tracks.iter().filter_map(|t| t.quality.as_deref()).collect();
    labels.sort_unstable();
    labels.dedup();
    match labels.as_slice() {
        [] => None,
        [one] => Some((*one).to_owned()),
        many => Some(many.join(" · ")),
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::Artist;

    use super::*;

    #[test]
    fn artist_cards_carry_the_stored_photo_when_the_file_exists() {
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let with = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let gone = lib.upsert_artist(&Artist::new("Земфира")).unwrap();
        let none = lib.upsert_artist(&Artist::new("Nobody")).unwrap();
        std::fs::write(dir.path().join("kino.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
        // One absolute path would also do; this one is relative to the library root.
        lib.set_artist_photo(with.id, Some("kino.jpg")).unwrap();
        lib.set_artist_photo(gone.id, Some("missing.jpg")).unwrap();
        lib.set_artist_photo(none.id, None).unwrap();

        let mut res = Resolver::new(lib.clone(), dir.path().to_owned());
        let card = |res: &mut Resolver, a: &nikgob_core::model::Artist| {
            let row = ArtistRow { artist: a.clone(), release_count: 0, local_track_count: 0 };
            res.artist(&row)
        };
        assert_eq!(
            card(&mut res, &with).image.map(|c| c.path),
            Some(dir.path().join("kino.jpg"))
        );
        assert!(card(&mut res, &gone).image.is_none());
        assert!(card(&mut res, &none).image.is_none());

        // A photo that arrives later shows up after the cache is dropped.
        std::fs::write(dir.path().join("late.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
        lib.set_artist_photo(none.id, Some("late.jpg")).unwrap();
        assert!(card(&mut res, &none).image.is_none());
        res.invalidate_photos();
        assert!(card(&mut res, &none).image.is_some());
    }

    #[test]
    fn a_stream_cover_url_passes_through_until_a_file_replaces_it() {
        use nikgob_core::model::{ArtistCredit, Release};
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut r = Release::new("Группа крови", ArtistCredit::single("Кино", None));
        r.cover_ref = Some("https://i.scdn.co/image/abc".into());
        let r = lib.upsert_release(&r).unwrap();
        let mut res = Resolver::new(lib.clone(), dir.path().to_owned());
        let cover = res.release_card(&r, 0, Some(4), false).cover.unwrap();
        assert_eq!(cover.uri(), "https://i.scdn.co/image/abc");
        assert_eq!(cover.web_url(), Some("https://i.scdn.co/image/abc"));
        assert!(cover.file().is_none() && cover.ambient.is_none());

        let mut local = r.clone();
        local.cover_ref = Some("Кино/cover.jpg".into());
        let local = lib.upsert_release(&local).unwrap();
        res.invalidate();
        let cover = res.release_card(&local, 1, Some(4), false).cover.unwrap();
        assert!(cover.uri().starts_with("file://"));
        assert_eq!(cover.file(), Some(dir.path().join("Кино/cover.jpg").as_path()));
    }

    #[test]
    fn a_stream_artist_image_shows_when_there_is_no_photo() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        assert!(lib.set_stream_artist_image(a.id, "https://fake.invalid/kino.jpg").unwrap());
        assert!(!lib.set_stream_artist_image(a.id, "https://fake.invalid/kino.jpg").unwrap());
        assert!(!lib.set_stream_artist_image(a.id, "C:/kino.jpg").unwrap());
        let a = lib.get_artist(a.id).unwrap().unwrap();
        let mut res = Resolver::new(lib.clone(), std::env::temp_dir());
        let row = ArtistRow { artist: a, release_count: 0, local_track_count: 0 };
        let image = res.artist(&row).image.unwrap();
        assert_eq!(image.web_url(), Some("https://fake.invalid/kino.jpg"));
    }
}
