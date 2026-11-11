//! Library rows → view rows: resolves cover paths, palette seeds and the
//! quality label. Caches per-release cover lookups.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nikgob_core::model::{ArtistId, Codec, PlaylistId, Release, ReleaseId, Source, SourceKind};
use nikgob_library::{
    ArtistRow, Library, ListingEntry, PlaylistKind, PlaylistSummary, ReleaseListing, ReleaseProgress, ReleaseRow, TrackRow,
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
    /// Palette seeds of every release, read in one query on the first cover
    /// miss and dropped with `covers`.
    seeds: Option<HashMap<ReleaseId, u32>>,
    /// Artist photos on disk; loaded on first use.
    photos: Option<HashMap<ArtistId, v::Cover>>,
    /// Readable names of artists and their roles (`artist_latin`), loaded
    /// on first use and dropped with `photos`.
    names: Option<HashMap<ArtistId, (Option<String>, bool)>>,
    /// How many times the covers and the photos were invalidated, shared
    /// with every resolver forked from this one ([`Self::fork`]): one
    /// invalidated anywhere drops its caches before it reads them next.
    stale: Arc<Stale>,
    /// The counts of `stale` this resolver's caches are of.
    seen: (u64, u64),
    /// What the files played lately say of their sound (shared with the
    /// forks and with the threads that read them ahead).
    facts: FactsCache,
}

/// See [`Resolver::stale`].
#[derive(Default)]
struct Stale {
    covers: AtomicU64,
    photos: AtomicU64,
}

/// What playing a file needs of its tags, read once with them: its track
/// gain (R128 or ReplayGain, dB; 0 without one), its channels and the tags
/// themselves without the cover picture (the info panel and the embedded
/// lyrics read them when it starts).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileFacts {
    pub gain_db: f32,
    pub channels: Option<u8>,
    pub tags: Option<Arc<nikgob_import::tags::TagSet>>,
}

/// [`FileFacts`] kept this many files at most (then forgotten at once).
const FACTS_KEPT: usize = 256;

/// The [`FileFacts`] of the files played lately, each with the file's
/// modification time then (a file written since is read again). A start
/// reads its file's tags here at most once: the gain for the engine, the
/// channels for «Источник и качество», the tags for the info panel and the
/// lyrics; the next track's are read ahead off the loop.
#[derive(Clone, Default)]
pub(crate) struct FactsCache(Arc<parking_lot::Mutex<HashMap<PathBuf, (Option<std::time::SystemTime>, FileFacts)>>>);

impl FactsCache {
    /// The facts of `path` if they are kept and the file is as it was.
    pub(crate) fn get(&self, path: &Path) -> Option<FileFacts> {
        let (at, facts) = self.0.lock().get(path).cloned()?;
        (at == modified(path)).then_some(facts)
    }

    /// The facts of `path`: kept, or read now and kept.
    pub(crate) fn of(&self, path: &Path) -> FileFacts {
        if let Some(f) = self.get(path) {
            return f;
        }
        let at = modified(path);
        // A file that breaks the tag reader is kept as one without tags,
        // not read again and again (nor left half read by its thread).
        let facts = std::panic::catch_unwind(|| read_facts(path)).unwrap_or_else(|_| {
            log::warn!("{}: its tags could not be read", path.display());
            FileFacts::default()
        });
        let mut kept = self.0.lock();
        if kept.len() >= FACTS_KEPT {
            kept.clear();
        }
        kept.insert(path.to_owned(), (at, facts.clone()));
        facts
    }

    /// The tags of `path` (without the cover picture): kept, or read now.
    pub(crate) fn tags(&self, path: &Path) -> Option<nikgob_import::tags::TagSet> {
        self.of(path).tags.map(|t| (*t).clone())
    }

    /// A guess at the gain of `path` before its tags are read: the middle
    /// one of the gains of the files beside it read lately (an album's
    /// tracks are mastered alike), else of every file read lately, else
    /// none (0 dB). Files whose tags did not read do not count.
    pub(crate) fn guess_gain(&self, path: &Path) -> f32 {
        let kept = self.0.lock();
        let read = kept.iter().filter(|(_, (_, f))| f.tags.is_some());
        let dir = path.parent();
        let beside: Vec<f32> = read
            .clone()
            .filter(|(p, _)| p.parent() == dir)
            .map(|(_, (_, f))| f.gain_db)
            .collect();
        let gains = if beside.is_empty() { read.map(|(_, (_, f))| f.gain_db).collect() } else { beside };
        median(gains).unwrap_or(0.0)
    }
}

/// The middle value of `values` (the mean of the two middle ones for an
/// even count); none of none.
fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let m = values.len() / 2;
    Some(if values.len() % 2 == 0 { (values[m - 1] + values[m]) / 2.0 } else { values[m] })
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Reads the [`FileFacts`] of `path` from its tags (none when it does not
/// read). The gain is normalized to −18 LUFS (the ReplayGain 2.0
/// reference; R128 gains are relative to −23 LUFS).
fn read_facts(path: &Path) -> FileFacts {
    let Ok((mut tags, info)) = nikgob_import::tags::read_tags(path) else {
        return FileFacts::default();
    };
    // The picture is large and nobody here wants it.
    tags.cover = None;
    FileFacts {
        gain_db: tags
            .r128_track_gain
            .map(|q| f32::from(q) / 256.0 + 5.0)
            .or(tags.replaygain_track_gain_db)
            .unwrap_or(0.0),
        channels: info.channels,
        tags: Some(Arc::new(tags)),
    }
}

impl Resolver {
    pub fn new(lib: Library, root: PathBuf) -> Self {
        Self {
            lib,
            root,
            covers: HashMap::new(),
            seeds: None,
            photos: None,
            names: None,
            stale: Arc::new(Stale::default()),
            seen: (0, 0),
            facts: FactsCache::default(),
        }
    }

    /// A resolver with caches of its own for another thread (the home's
    /// builder): an invalidation of either drops the caches of both.
    pub(crate) fn fork(&self) -> Self {
        Self {
            lib: self.lib.clone(),
            root: self.root.clone(),
            covers: HashMap::new(),
            seeds: None,
            photos: None,
            names: None,
            stale: self.stale.clone(),
            seen: self.stale_now(),
            facts: self.facts.clone(),
        }
    }

    /// The library this resolver reads.
    pub(crate) fn library(&self) -> &Library {
        &self.lib
    }

    /// What the files played lately say of their sound ([`FactsCache`]).
    pub(crate) fn facts(&self) -> &FactsCache {
        &self.facts
    }

    fn stale_now(&self) -> (u64, u64) {
        (self.stale.covers.load(Ordering::Acquire), self.stale.photos.load(Ordering::Acquire))
    }

    /// Drops what an invalidation (here or in a fork) made stale.
    fn sync(&mut self) {
        let now = self.stale_now();
        if now.0 != self.seen.0 {
            self.covers.clear();
            self.seeds = None;
        }
        if now.1 != self.seen.1 {
            self.photos = None;
            self.names = None;
        }
        self.seen = now;
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
        self.sync();
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
        // Real art found for a poor cover (a clip's frame, a tiny file).
        if let Some(url) = crate::better_covers::cached(&self.lib, crate::better_covers::Target::Release(id), cover_ref.as_deref()) {
            let cover = Some(v::Cover::web(url));
            self.covers.insert(id, cover.clone());
            return cover;
        }
        let cover = cover_ref.map(|r| {
            // A stream album's cover until a file brings one.
            if nikgob_library::is_web_ref(&r) {
                return v::Cover::web(r);
            }
            let path = self.path(&r);
            let ambient = path.with_file_name("ambient.png");
            let seeds = self
                .seeds
                .get_or_insert_with(|| self.lib.palette_seeds().unwrap_or_default());
            // One stored after the seeds were read is looked up on its own.
            let seed = seeds.get(&id).copied().or_else(|| {
                let seed = self.lib.get_palette(id).ok().flatten().map(|p| p.seed_argb)?;
                seeds.insert(id, seed);
                Some(seed)
            });
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
        self.sync();
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

    /// The Latin name to show beside `name` (only a name in a foreign
    /// script has one) and whether the artist is a role.
    pub fn latin_name(&mut self, id: ArtistId, name: &str) -> (Option<String>, bool) {
        self.sync();
        let names = self.names.get_or_insert_with(|| self.lib.artist_latin_names().unwrap_or_default());
        let Some((latin, role)) = names.get(&id) else { return (None, false) };
        let shown = latin
            .clone()
            .filter(|l| nikgob_core::names::foreign_script(name) && nikgob_core::names::name_key(l) != nikgob_core::names::name_key(name));
        (shown, *role)
    }

    /// Reload artist photos (and readable names) from the library on next
    /// use (in the forks too).
    pub fn invalidate_photos(&mut self) {
        self.stale.photos.fetch_add(1, Ordering::AcqRel);
        self.sync();
    }

    /// Forget cached covers (after an import changed them), in the forks too.
    pub fn invalidate(&mut self) {
        self.stale.covers.fetch_add(1, Ordering::AcqRel);
        self.sync();
    }

    /// The cover of a release (its file, or a service's https url).
    pub fn release_cover(&mut self, id: ReleaseId) -> Option<v::Cover> {
        self.cover_for(Some(id), None)
    }

    /// A world with its stage cover (and the larger one, if one was found).
    pub fn world(&mut self, w: &v::World) -> v::World {
        v::World {
            cover: w.release.and_then(|r| self.release_cover(r)),
            cover_hd: w.release.and_then(|r| crate::hd_covers::cached(&self.lib, r)).map(v::Cover::web),
            ..w.clone()
        }
    }

    /// A world's page: its albums with how much of each was heard, its new
    /// album of the services and its liked tracks.
    pub fn world_view(&mut self, w: &crate::home_v2::WorldParts, new_album: Option<v::StreamAlbumCard>) -> v::WorldView {
        let albums = w
            .albums
            .iter()
            .map(|(r, heard, total)| v::WorldAlbum { card: self.release(r), heard: *heard, total: *total })
            .collect();
        let rows: Vec<TrackRow> = w.liked.iter().filter_map(|t| self.lib.get_track_row(*t).ok().flatten()).collect();
        let liked = rows.iter().map(|t| self.track(t)).collect();
        v::WorldView { world: self.world(&w.world), albums, new_album, liked, mixes: Vec::new() }
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
            // Honest about a "FLAC" plainly made from an MP3 («FLAC из MP3
            // ~192»); a doubt is said only in the quality panel.
            quality: r.codec.as_ref().map(|c| nikgob_core::quality::row_label(c, r.check.as_ref())),
            source: r.active_kind,
            has_local: r.has_local,
            liked: r.liked,
            // A clip has no album: its song's art when it was found, else
            // the video's own picture. So has a song YouTube Music named no
            // album for (its top search result) until its details come: its
            // picture is the album art between black bars, cut out on load.
            cover: self.cover_for(r.release_id, r.cover_ref.as_deref()).or_else(|| {
                let video = r.youtube_id.as_deref()?;
                let art = r
                    .is_clip()
                    .then(|| crate::better_covers::cached(&self.lib, crate::better_covers::Target::Track(r.id), None))
                    .flatten();
                Some(v::Cover::web(art.unwrap_or_else(|| format!("https://i.ytimg.com/vi/{video}/hqdefault.jpg"))))
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
            added_at: r.added_at,
            better: local > 0 && self.lib.release_can_get_better(r.id).unwrap_or(false),
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
        let (latin, character) = self.latin_name(a.artist.id, &a.artist.name);
        v::ArtistCard {
            id: a.artist.id,
            name: a.artist.name.clone(),
            image: self.photo(a.artist.id).or_else(|| web_image(&a.artist.images)),
            liked: a.liked,
            track_count: a.local_track_count,
            latin,
            character,
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

    /// A collage / smart cover from the playlist's album covers: of their
    /// files, else (a playlist of stream songs) the stream cover of its most
    /// common album, a gradient only when no album has a cover.
    fn auto_cover(&mut self, id: PlaylistId, mode: v::PlaylistCoverMode) -> Option<v::Cover> {
        let all = self.lib.playlist_cover_refs(id).unwrap_or_default();
        // Covers whose file is gone count as none (checked once per album).
        let mut exists: HashMap<ReleaseId, bool> = HashMap::new();
        let refs: Vec<(ReleaseId, PathBuf)> = all
            .iter()
            .filter(|(_, c)| !nikgob_library::is_web_ref(c))
            .map(|(r, c)| (*r, self.path(c)))
            .filter(|(r, p)| *exists.entry(*r).or_insert_with(|| p.exists()))
            .collect();
        let plain = |path: PathBuf| v::Cover { path, seed: None, ambient: None };
        let web: Vec<(ReleaseId, PathBuf)> = all
            .iter()
            .filter(|(_, c)| nikgob_library::is_web_ref(c))
            .map(|(r, c)| (*r, PathBuf::from(c)))
            .collect();
        match playlist_cover::pick(&refs, mode) {
            playlist_cover::Pick::Single(r) => self.cover_for(Some(r), None),
            playlist_cover::Pick::Collage(files) => playlist_cover::collage(&self.root, &files).map(plain),
            // A web cover is drawn as it is, never in a collage: the most
            // common album's (what `pick` gives outside smart and collage).
            playlist_cover::Pick::Placeholder => match playlist_cover::pick(&web, v::PlaylistCoverMode::Custom) {
                playlist_cover::Pick::Single(r) => self.cover_for(Some(r), None),
                _ => playlist_cover::placeholder(&self.root, &id.to_string()).map(plain),
            },
        }
    }

    pub fn release_detail(
        &mut self,
        r: &Release,
        listing: &ReleaseListing,
        liked: bool,
    ) -> v::ReleaseDetail {
        let (local, total) = (listing.progress.local, Some(listing.progress.total));
        let tracks: Vec<v::TrackRow> = listing.tracks().map(|t| self.track(t)).collect();
        let mut at = 0;
        let lines = listing
            .entries
            .iter()
            .map(|e| match e {
                ListingEntry::Track(_) => {
                    at += 1;
                    v::ReleaseLine::Track(at - 1)
                }
                ListingEntry::Hole { disc, no } => v::ReleaseLine::Hole { disc: *disc, no: *no },
            })
            .collect();
        let total_duration = tracks.iter().filter_map(|t| t.duration).sum();
        let quality = summarize_quality(&tracks);
        v::ReleaseDetail {
            card: self.release_card(r, local, total, liked),
            tracks,
            total_duration,
            quality,
            liked,
            lines,
            // From the files' tags, filled by the page's load.
            released: None,
            label: None,
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
        let (latin, character) = self.latin_name(a.id, &a.name);
        v::ArtistDetail {
            card: v::ArtistCard {
                id: a.id,
                name: a.name.clone(),
                image: self.photo(a.id).or_else(|| web_image(&a.images)),
                liked,
                track_count: 0,
                latin,
                character,
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
        // What the spectrum check found about a file (a stream has none).
        // A lossy file's own lowpass is its format's: only a check of
        // lossless sound counts (its own, or the original's it was made from).
        let check = src
            .filter(|s| s.kind == SourceKind::LocalFile)
            .and_then(|s| {
                self.lib
                    .quality_check(s.id)
                    .ok()
                    .flatten()
                    .filter(|c| nikgob_core::quality::check_counts(&s.codec, c.made_from.as_ref()))
            });
        let quality = src.map(|s| {
            // «FLAC из MP3 ~320», «MP3 320 из поддельного FLAC», «Opus 160».
            let mut q = nikgob_core::quality::label(&s.codec, s.bitrate, check.as_ref().map(|c| c.summary()).as_ref());
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
        let sound = src.map(|s| {
            let path = (s.kind == SourceKind::LocalFile).then(|| self.path(&s.locator));
            // Read with its gain when it started (once per file, off the
            // loop: until then without its channels).
            let channels = path.as_deref().and_then(|p| self.facts.get(p)).and_then(|f| f.channels);
            let mut d = sound_details(s, path.as_deref(), channels, row.duration_ms, check.as_ref());
            // A check of another file than the one there now: no MP3 until
            // the file is checked again.
            if d.to_mp3 && !check.as_ref().zip(path.as_deref()).is_some_and(|(c, p)| crate::quality::check_fresh(p, c)) {
                d.to_mp3 = false;
            }
            d
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
                    SourceKind::Soundcloud => "SoundCloud",
                }
                .to_owned()
            }),
            path: src
                .filter(|s| s.kind == SourceKind::LocalFile)
                .map(|s| self.path(&s.locator)),
            tags,
            sound,
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
            history: h.v2.history,
            shortcuts: h
                .v2
                .shortcuts
                .iter()
                .map(|s| match s {
                    crate::home_v2::ShortcutRef::Release(r) => v::Shortcut::Release(self.release(r)),
                    crate::home_v2::ShortcutRef::Artist(a) => v::Shortcut::Artist(self.artist(a)),
                    crate::home_v2::ShortcutRef::Playlist(p) => {
                        let mut covers = Vec::new();
                        if let Some(id) = p.id {
                            for (r, _) in self.lib.playlist_cover_refs(id).unwrap_or_default() {
                                if covers.len() == 4 {
                                    break;
                                }
                                if let Some(c) = self.release_cover(r)
                                    && !covers.contains(&c)
                                {
                                    covers.push(c);
                                }
                            }
                        }
                        v::Shortcut::Playlist(self.playlist(p), covers)
                    }
                })
                .collect(),
            listen_again: h.v2.listen_again.iter().map(|t| self.track(t)).collect(),
            from_library: h
                .v2
                .from_library
                .iter()
                .map(|(r, heard, total)| v::ShelfAlbum { card: self.release(r), heard: *heard, total: *total })
                .collect(),
            worlds: h
                .v2
                .worlds
                .iter()
                .map(|w| self.world(&w.world))
                .collect(),
            // The recommendations' parts come from `recs`.
            ..v::HomeView::default()
        }
    }
}

pub fn codec_label(c: &Codec) -> String {
    nikgob_core::quality::codec_name(c)
}

/// "44,1 кГц".
fn fmt_rate(hz: u32) -> String {
    if hz % 1000 == 0 {
        format!("{} кГц", hz / 1000)
    } else {
        format!("{:.1} кГц", f64::from(hz) / 1000.0).replace('.', ",")
    }
}

/// "25,5 МБ".
fn fmt_mb(bytes: u64) -> String {
    format!("{:.1} МБ", bytes as f64 / 1_048_576.0).replace('.', ",")
}

/// The container of a file, by its extension.
fn container_of(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(
        match ext.as_str() {
            "flac" => "FLAC",
            "opus" | "ogg" | "oga" => "Ogg",
            "mp3" => "MP3",
            "m4a" | "mp4" | "aac" => "MP4",
            "wav" => "WAV",
            "aif" | "aiff" => "AIFF",
            "webm" | "mka" => "WebM",
            other => return Some(other.to_uppercase()),
        }
        .to_owned(),
    )
}

/// The «Качество» section of a track's info: the facts of `src` (a
/// file's `path` and `channels` when it is one) and its check.
pub(crate) fn sound_details(
    src: &Source,
    path: Option<&Path>,
    channels: Option<u8>,
    duration_ms: Option<u32>,
    check: Option<&nikgob_core::quality::QualityCheck>,
) -> v::SoundDetails {
    use nikgob_core::quality::{CheckSummary, cutoff_text, verdict_text};
    let mut facts = vec![("Кодек".to_owned(), codec_label(&src.codec))];
    if let Some(c) = path.and_then(container_of) {
        facts.push(("Контейнер".into(), c));
    }
    if let Some(b) = src.bitrate {
        facts.push(("Битрейт".into(), format!("{b} кбит/с")));
    }
    if let Some(sr) = src.sample_rate {
        facts.push(("Частота".into(), fmt_rate(sr)));
    }
    if let Some(bd) = src.bit_depth {
        facts.push(("Разрядность".into(), format!("{bd} бит")));
    }
    if let Some(ch) = channels {
        let c = match ch {
            1 => "моно".to_owned(),
            2 => "стерео".to_owned(),
            n => n.to_string(),
        };
        facts.push(("Каналы".into(), c));
    }
    if let Some(ms) = duration_ms {
        let s = ms / 1000;
        facts.push(("Длительность".into(), format!("{}:{:02}", s / 60, s % 60)));
    }
    if let Some(size) = src.size_bytes.filter(|_| path.is_some()) {
        facts.push(("Размер".into(), fmt_mb(size)));
    }
    let from = match (&src.kind, &src.origin) {
        (SourceKind::LocalFile, nikgob_core::model::SourceOrigin::Torrent { .. }) => "Торрент",
        (SourceKind::LocalFile, nikgob_core::model::SourceOrigin::Stream) => "Сохранён со стрима",
        (SourceKind::LocalFile, nikgob_core::model::SourceOrigin::Inbox) => "Скачан или добавлен файлом",
        (SourceKind::Spotify, _) => "Поток Spotify",
        (SourceKind::Youtube | SourceKind::Deezer, _) => "Поток YouTube Music",
        (SourceKind::Soundcloud, _) => "Поток SoundCloud",
    };
    facts.push(("Откуда".into(), from.to_owned()));
    let local = src.kind == SourceKind::LocalFile;
    // A plain fake is a warning; a doubt is worded as one and is none.
    let warning = check.is_some_and(|c| {
        let s: CheckSummary = c.summary();
        nikgob_core::quality::plain_fake(&src.codec, &s)
    });
    v::SoundDetails {
        facts,
        verdict: check.map(|c| verdict_text(&src.codec, src.bitrate, c)),
        warning,
        // A file made from another: the band is the original's.
        band: check.and_then(|c| {
            let t = cutoff_text(c)?;
            Some(if c.made_from.is_some() { format!("{t} (у исходника)") } else { t })
        }),
        spectrum: check.filter(|c| !c.spectrum_db.is_empty() && c.sample_rate > 0).map(|c| v::SpectrumPlot {
            db: c.spectrum_db.clone(),
            nyquist_hz: c.sample_rate as f32 / 2.0,
            cutoff_hz: (c.verdict != nikgob_core::quality::Verdict::Unknown && c.cutoff_hz > 0.0).then_some(c.cutoff_hz),
        }),
        // Only lossless files are checked.
        pending: local && src.codec.is_lossless() && check.is_none(),
        // A live file that surely holds an MP3; a doubt is offered only
        // the search for a better copy.
        to_mp3: local
            && src.trashed_at.is_none()
            && check.is_some_and(|c| nikgob_core::quality::mp3_inside(&src.codec, &c.summary())),
        // The track's file, whichever plays now (the one in the Trash
        // after its MP3 took its place, until the next track).
        better: local,
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
    fn a_gain_is_guessed_from_the_files_beside_it_then_from_all() {
        let cache = FactsCache::default();
        assert_eq!(cache.guess_gain(Path::new("m/a/01.flac")), 0.0);
        let keep = |path: &str, gain_db: f32, read: bool| {
            let facts = FileFacts {
                gain_db,
                channels: None,
                tags: read.then(|| Arc::new(nikgob_import::tags::TagSet::default())),
            };
            cache.0.lock().insert(PathBuf::from(path), (None, facts));
        };
        keep("m/a/02.flac", -8.0, true);
        keep("m/a/03.flac", -6.0, true);
        keep("m/a/04.flac", -7.0, true);
        keep("m/b/01.flac", 2.0, true);
        keep("m/b/02.flac", 4.0, true);
        // A file whose tags did not read says nothing.
        keep("m/a/05.flac", 0.0, false);
        assert_eq!(cache.guess_gain(Path::new("m/a/01.flac")), -7.0);
        assert_eq!(cache.guess_gain(Path::new("m/b/03.flac")), 3.0);
        // Nothing beside it: the middle of all of them.
        assert_eq!(cache.guess_gain(Path::new("m/c/01.flac")), -6.0);
    }

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
            let row = ArtistRow { artist: a.clone(), release_count: 0, local_track_count: 0, liked: false };
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

    /// A playlist of stream songs shows their album's cover, not a gradient
    /// (an imported playlist of songs the library has no file of).
    #[test]
    fn a_playlist_of_streams_takes_their_cover() {
        use nikgob_core::model::{ArtistCredit, Release, Track};
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut ids = Vec::new();
        for (album, cover) in [("A", Some("https://i.ytimg.com/a.jpg")), ("A", Some("https://i.ytimg.com/a.jpg")), ("B", Some("https://i.ytimg.com/b.jpg")), ("C", None)] {
            let mut r = Release::new(album, ArtistCredit::single("X", None));
            r.cover_ref = cover.map(str::to_owned);
            let r = lib.upsert_release(&r).unwrap();
            let mut t = Track::new(album, ArtistCredit::single("X", None));
            t.release_id = Some(r.id);
            ids.push(lib.upsert_track(&t).unwrap().id);
        }
        let p = lib.create_playlist("Streams").unwrap();
        lib.playlist_add_many(p.id, &ids).unwrap();
        let summary = lib.list_playlists().unwrap().into_iter().find(|s| s.id == Some(p.id)).unwrap();
        let mut res = Resolver::new(lib.clone(), dir.path().to_owned());
        let cover = res.playlist(&summary).cover.unwrap();
        assert_eq!(cover.web_url(), Some("https://i.ytimg.com/a.jpg"), "the most common album's");

        let empty = lib.create_playlist("Bare").unwrap();
        lib.playlist_add_many(empty.id, &ids[3..]).unwrap();
        let summary = lib.list_playlists().unwrap().into_iter().find(|s| s.id == Some(empty.id)).unwrap();
        let cover = res.playlist(&summary).cover.unwrap();
        assert!(cover.web_url().is_none(), "no cover anywhere: the gradient");
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
        let row = ArtistRow { artist: a, release_count: 0, local_track_count: 0, liked: false };
        let image = res.artist(&row).image.unwrap();
        assert_eq!(image.web_url(), Some("https://fake.invalid/kino.jpg"));
    }

    /// The home's builder has a resolver of its own: a cover changed by an
    /// import and invalidated on the worker is read again there too.
    #[test]
    fn an_invalidation_drops_the_caches_of_a_fork_too() {
        use nikgob_core::model::{ArtistCredit, Release};
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut r = Release::new("A", ArtistCredit::single("X", None));
        r.cover_ref = Some("https://fake.invalid/a.jpg".into());
        let r = lib.upsert_release(&r).unwrap();
        let mut res = Resolver::new(lib.clone(), dir.path().to_owned());
        let mut fork = res.fork();
        let url = |res: &mut Resolver| res.release_cover(r.id).and_then(|c| c.web_url().map(str::to_owned));
        assert_eq!(url(&mut fork).as_deref(), Some("https://fake.invalid/a.jpg"));
        let mut changed = r.clone();
        changed.cover_ref = Some("https://fake.invalid/b.jpg".into());
        lib.upsert_release(&changed).unwrap();
        assert_eq!(url(&mut fork).as_deref(), Some("https://fake.invalid/a.jpg"), "cached");
        res.invalidate();
        assert_eq!(url(&mut fork).as_deref(), Some("https://fake.invalid/b.jpg"));
        assert_eq!(url(&mut res).as_deref(), Some("https://fake.invalid/b.jpg"));
    }

    fn check(verdict: nikgob_core::quality::Verdict, cutoff: f32, made_from: Option<Codec>) -> nikgob_core::quality::QualityCheck {
        nikgob_core::quality::QualityCheck {
            verdict,
            cutoff_hz: cutoff,
            steepness_db: 45.0,
            origin: nikgob_core::quality::SoundOrigin::estimate(verdict, cutoff),
            sample_rate: 44_100,
            spectrum_db: vec![-40.0; nikgob_core::quality::SPECTRUM_POINTS],
            made_from,
            file_size: 1,
            file_mtime: 1,
            checked_at: 1,
        }
    }

    #[test]
    fn the_quality_section_lists_the_facts_and_the_honest_verdict() {
        use nikgob_core::model::TrackId;
        use nikgob_core::quality::Verdict;
        let mut flac = Source::new(TrackId::new(), SourceKind::LocalFile, "a/07 - Молли.flac", Codec::Flac);
        flac.sample_rate = Some(44_100);
        flac.bit_depth = Some(16);
        flac.bitrate = Some(1058);
        flac.size_bytes = Some(26_733_824);
        let fake = check(Verdict::Transcoded, 16_000.0, None);
        let d = sound_details(&flac, Some(Path::new("a/07 - Молли.flac")), Some(2), Some(152_790), Some(&fake));
        let facts: Vec<(&str, &str)> = d.facts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            facts,
            vec![
                ("Кодек", "FLAC"),
                ("Контейнер", "FLAC"),
                ("Битрейт", "1058 кбит/с"),
                ("Частота", "44,1 кГц"),
                ("Разрядность", "16 бит"),
                ("Каналы", "стерео"),
                ("Длительность", "2:32"),
                ("Размер", "25,5 МБ"),
                ("Откуда", "Скачан или добавлен файлом"),
            ]
        );
        assert!(d.warning);
        assert!(d.verdict.as_deref().unwrap().starts_with("Не настоящий lossless"));
        assert_eq!(d.band.as_deref(), Some("до 16,0 кГц"));
        let plot = d.spectrum.unwrap();
        assert_eq!((plot.nyquist_hz, plot.cutoff_hz), (22_050.0, Some(16_000.0)));
        assert!(!d.pending);
        assert!(d.to_mp3 && d.better, "a sure fake: its MP3 and a better copy");

        // A real one: no warning, the full band; not checked yet: pending.
        let real = sound_details(&flac, Some(Path::new("x.flac")), None, None, Some(&check(Verdict::Genuine, 22_050.0, None)));
        assert!(!real.warning);
        assert_eq!(real.band.as_deref(), Some("до 22,0 кГц, без среза"));
        // A cut a real master may have too: a doubt in words, no warning.
        let doubt = sound_details(&flac, Some(Path::new("x.flac")), None, None, Some(&check(Verdict::Transcoded, 20_155.0, None)));
        assert!(!doubt.warning);
        assert!(doubt.verdict.as_deref().unwrap().starts_with("Возможно, сделан из MP3 ~320"));
        assert!(!doubt.to_mp3 && doubt.better, "a doubt: only the search");
        assert!(!real.to_mp3 && real.better);
        let unchecked = sound_details(&flac, Some(Path::new("x.flac")), None, None, None);
        assert!(unchecked.pending && unchecked.verdict.is_none() && unchecked.spectrum.is_none());

        // The MP3 a fake became: honest, and the band is the original's.
        let mut mp3 = Source::new(TrackId::new(), SourceKind::LocalFile, "a/07 - Молли.mp3", Codec::Mp3);
        mp3.bitrate = Some(320);
        let conv = sound_details(&mp3, Some(Path::new("a/07 - Молли.mp3")), None, None, Some(&check(Verdict::Transcoded, 16_000.0, Some(Codec::Flac))));
        assert!(conv.warning);
        assert!(conv.verdict.as_deref().unwrap().starts_with("Сделан из поддельного FLAC"));
        assert_eq!(conv.band.as_deref(), Some("до 16,0 кГц (у исходника)"));
        assert!(!conv.to_mp3, "an MP3 already");
        // The original in the Trash (it may still be playing): nothing to do.
        let mut gone = flac.clone();
        gone.trashed_at = Some(1);
        let gone = sound_details(&gone, Some(Path::new("x.flac")), None, None, Some(&fake));
        assert!(!gone.to_mp3 && gone.better);
        // A lossy file is never checked: nothing pending, no verdict.
        let plain = sound_details(&mp3, Some(Path::new("RuLe.mp3")), None, None, None);
        assert!(!plain.pending && plain.verdict.is_none());

        // A stream: what it gives, nothing to check.
        let mut yt = Source::new(TrackId::new(), SourceKind::Youtube, "abc", Codec::Opus);
        yt.bitrate = Some(160);
        let s = sound_details(&yt, None, None, Some(200_000), None);
        assert!(!s.pending && s.verdict.is_none());
        assert!(!s.to_mp3 && !s.better, "nothing to do with a stream's file");
        assert!(s.facts.contains(&("Откуда".into(), "Поток YouTube Music".into())));
        assert!(!s.facts.iter().any(|(k, _)| k == "Размер" || k == "Контейнер"));
    }
}
