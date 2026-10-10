//! Library rows → view rows: resolves cover paths, palette seeds and the
//! quality label. Caches per-release cover lookups.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_core::model::{Codec, Release, ReleaseId, Source, SourceKind};
use nikgob_library::{
    ArtistRow, Library, PlaylistKind, PlaylistSummary, ReleaseProgress, ReleaseRow, TrackRow,
};
use nikgob_view as v;

use crate::home::Home;

pub struct Resolver {
    lib: Library,
    root: PathBuf,
    covers: HashMap<ReleaseId, Option<v::Cover>>,
}

impl Resolver {
    pub fn new(lib: Library, root: PathBuf) -> Self {
        Self {
            lib,
            root,
            covers: HashMap::new(),
        }
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

    /// Forget cached covers (after an import changed them).
    pub fn invalidate(&mut self) {
        self.covers.clear();
    }

    pub fn track(&mut self, r: &TrackRow) -> v::TrackRow {
        v::TrackRow {
            id: r.id,
            title: r.title.clone(),
            artist: r.artist.clone(),
            album: r.album.clone(),
            release_id: r.release_id,
            artist_id: r.artist_id,
            disc_no: r.disc_no,
            track_no: r.track_no,
            duration: r.duration_ms.map(|ms| Duration::from_millis(u64::from(ms))),
            quality: r.codec.as_ref().map(codec_label),
            source: r.active_kind,
            has_local: r.has_local,
            liked: r.liked,
            cover: self.cover_for(r.release_id, r.cover_ref.as_deref()),
        }
    }

    pub fn release_card(&mut self, r: &Release, local: u32, total: Option<u32>) -> v::ReleaseCard {
        v::ReleaseCard {
            id: r.id,
            title: r.title.clone(),
            artist: r.artist_credit.display(),
            artist_id: r.artist_credit.0.first().and_then(|p| p.artist_id),
            year: r.year,
            release_type: r.release_type,
            cover: self.cover_for(Some(r.id), r.cover_ref.as_deref()),
            local_tracks: local,
            total_tracks: total,
            liked: self.lib.is_liked(r.id).unwrap_or(false),
        }
    }

    pub fn release(&mut self, r: &ReleaseRow) -> v::ReleaseCard {
        let p = r.progress();
        self.release_card(&r.release, p.local, Some(p.total))
    }

    pub fn artist(&mut self, a: &ArtistRow) -> v::ArtistCard {
        v::ArtistCard {
            id: a.artist.id,
            name: a.artist.name.clone(),
            image: None,
            liked: self.lib.is_liked(a.artist.id).unwrap_or(false),
        }
    }

    pub fn playlist(&mut self, p: &PlaylistSummary) -> v::PlaylistCard {
        v::PlaylistCard {
            id: p.id.unwrap_or(v::LIKED_PLAYLIST),
            name: p.name.clone(),
            cover: p.cover.as_ref().map(|c| v::Cover {
                path: self.path(c),
                seed: None,
                ambient: None,
            }),
            track_count: p.track_count,
            pinned: p.kind == PlaylistKind::Liked,
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
            card: self.release_card(r, local, total),
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
                image: None,
                liked,
            },
            top_tracks: top.iter().map(|t| self.track(t)).collect(),
            albums,
            singles_eps: singles,
            liked,
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
            continue_listening: h.continue_listening.iter().map(|r| self.release(r)).collect(),
            artist_mixes: h.artists.iter().map(|a| self.artist(a)).collect(),
            long_unplayed: h.long_unplayed.iter().map(|r| self.release(r)).collect(),
            recently_added: h.recently_added.iter().map(|r| self.release(r)).collect(),
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
