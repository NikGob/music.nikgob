//! The library's pages («Альбомы», «Исполнители», the overview, the shelf,
//! the first rows of «Треки») read off the player's loop.
//!
//! [`Command::LoadLibrary`] read every album and artist of the library and
//! made their cards on the worker's thread: half a second at a start, while
//! no click or playback event was answered and the artists page stayed
//! blank. Now the loop sends what to read to a thread of its own with a
//! resolver of its own ([`crate::Resolver::fork`]); the answer comes back
//! to the loop, which puts it in the view in one publish, as before.
//!
//! * **Newest wins.** Loads asked while one is read are one read: the
//!   newest. An answer to any but the newest load is dropped, so a slow
//!   read cannot put older rows over newer ones; an edit of the library
//!   while a load is on its way asks for a new one
//!   ([`Worker::library_load_pending`]) instead of patching rows the answer
//!   would put back.
//! * **Photos.** An artist photo saved while the cards were made is filled
//!   in when the answer is shown.
//!
//! [`Command::LoadLibrary`]: nikgob_view::Command::LoadLibrary

use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use nikgob_library::{Library, ReleaseSortKey, Sort, TrackSortKey};
use nikgob_view as v;

use crate::convert::Resolver;
use crate::{Later, Worker, home};

/// What a load reads.
struct Job {
    seq: u64,
    /// [`LibraryLoad::tracks_asked`] when the load was asked.
    tracks_asked: u64,
    release_sort: v::ReleaseSort,
    track_sort: v::TrackSort,
    only_local: bool,
}

/// The library's pages of load `seq`.
pub(crate) struct Loaded {
    seq: u64,
    tracks_asked: u64,
    releases: Vec<v::ReleaseCard>,
    playlists: Vec<v::PlaylistCard>,
    liked_albums: Vec<(usize, v::ReleaseCard)>,
    artists: Vec<v::ArtistCard>,
    recent: Vec<v::ReleaseCard>,
    often: Vec<v::ReleaseCard>,
    count: usize,
    /// The first rows of «Треки».
    tracks: Vec<v::TrackRow>,
    track_sort: v::TrackSort,
    /// What could not be read («библиотека», «исполнители»…): said as the
    /// loop said it.
    errors: Vec<&'static str>,
}

/// The worker's end of the loading thread.
pub(crate) struct LibraryLoad {
    /// `None`: no thread (it did not start or it stopped): read on the loop.
    jobs: Option<Sender<Job>>,
    /// The newest load asked.
    seq: u64,
    /// The newest load shown.
    shown: u64,
    /// Rows of «Треки» asked by themselves (a scroll, a sort): a load
    /// asked before the newest of them does not put its older rows back.
    pub(crate) tracks_asked: u64,
}

impl LibraryLoad {
    /// Starts the thread with a fork of the worker's resolver; answers go
    /// to the loop through `later`.
    pub(crate) fn start(res: &Resolver, later: Sender<Later>) -> Self {
        let (jobs, inbox) = crossbeam_channel::unbounded::<Job>();
        let fork = res.fork();
        let spawned = std::thread::Builder::new().name("nikgob-library".into()).spawn(move || run(&inbox, &later, fork));
        let jobs = match spawned {
            Ok(_) => Some(jobs),
            Err(e) => {
                log::error!("library: its loading thread did not start, the library is read on the loop: {e}");
                None
            }
        };
        Self { jobs, seq: 0, shown: 0, tracks_asked: 0 }
    }
}

fn run(inbox: &Receiver<Job>, later: &Sender<Later>, mut res: Resolver) {
    while let Ok(mut job) = inbox.recv() {
        while let Ok(newer) = inbox.try_recv() {
            job = newer;
        }
        let loaded = read(&mut res, &job);
        if later.send(Later::new("the library's pages", move |w| w.library_loaded(loaded))).is_err() {
            return;
        }
        // No read waits: what this one left goes back.
        if inbox.is_empty() {
            crate::release_thread_memory();
        }
    }
}

/// `r`, or nothing with `what` kept for the loop to say.
fn kept<T: Default>(errors: &mut Vec<&'static str>, what: &'static str, r: nikgob_library::Result<T>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            log::error!("{what}: {e}");
            errors.push(what);
            T::default()
        }
    }
}

/// The library's playlists and the albums liked as a whole among them
/// ([`v::LibraryView::liked_albums`]), both latest first, «Любимые»
/// staying first; under «только скачанное» the albums with a file.
pub(crate) fn shelf(
    lib: &Library,
    res: &mut Resolver,
    only_local: bool,
    errors: &mut Vec<&'static str>,
) -> (Vec<v::PlaylistCard>, Vec<(usize, v::ReleaseCard)>) {
    let playlists = kept(errors, "плейлисты", lib.list_playlists());
    let albums = kept(errors, "альбомы", lib.liked_releases(0, 10_000));
    let pinned = playlists.iter().take_while(|p| p.id.is_none()).count();
    let albums = albums
        .iter()
        .filter(|(r, _)| r.liked && (!only_local || r.local_tracks > 0))
        .map(|(r, at)| {
            let before = playlists.iter().skip(pinned).position(|p| p.updated_at < *at);
            (before.map_or(playlists.len(), |i| i + pinned), res.release(r))
        })
        .collect();
    let playlists = playlists.iter().map(|p| res.playlist(p)).collect();
    (playlists, albums)
}

/// Reads the library's pages of `job` and makes their cards.
fn read(res: &mut Resolver, job: &Job) -> Loaded {
    let started = Instant::now();
    let lib = res.library().clone();
    let only_local = job.only_local;
    let mut errors = Vec::new();
    let key = match job.release_sort {
        v::ReleaseSort::Artist => Sort::asc(ReleaseSortKey::Artist),
        v::ReleaseSort::Title => Sort::asc(ReleaseSortKey::Title),
        v::ReleaseSort::Year => Sort::desc(ReleaseSortKey::Year),
        v::ReleaseSort::Added => Sort::desc(ReleaseSortKey::Added),
    };
    let releases = kept(&mut errors, "библиотека", lib.releases(0, 20_000, key));
    let (playlists, liked_albums) = shelf(&lib, res, only_local, &mut errors);
    let artists = kept(
        &mut errors,
        "исполнители",
        lib.artists(0, 20_000, Sort::asc(nikgob_library::ArtistSortKey::Name)),
    );
    // The inconspicuous "только скачанное": what has a file.
    let (releases, artists): (Vec<_>, Vec<_>) = if only_local {
        (
            releases.into_iter().filter(|r| r.local_tracks > 0).collect(),
            artists.into_iter().filter(|a| a.local_track_count > 0).collect(),
        )
    } else {
        (releases, artists)
    };
    let count = lib.count_tracks(crate::track_filter(only_local)).unwrap_or(0) as usize;
    // The overview's rows: the latest albums in and the most played.
    let tracks = lib.rec_tracks().unwrap_or_default();
    let stats = lib.all_track_stats().unwrap_or_default();
    let per_artist = home::artist_track_counts(&tracks, only_local);
    let often = home::often_played(&tracks, &stats, 12);
    let mut recent: Vec<&nikgob_library::ReleaseRow> = releases.iter().collect();
    recent.sort_by_key(|r| std::cmp::Reverse(r.release.added_at));
    let brought = lib.releases_of_imported_playlists().unwrap_or_default();
    recent.retain(|r| !brought.contains(&r.release.id));
    recent.truncate(12);
    let by_id: std::collections::HashMap<_, _> = releases.iter().map(|r| (r.release.id, r)).collect();
    let often: Vec<&nikgob_library::ReleaseRow> = often.iter().filter_map(|id| by_id.get(id).copied()).collect();
    // By when they came, the albums only an import's playlists brought
    // stay out, as in «Недавно добавлено».
    let release_cards: Vec<_> = releases
        .iter()
        .filter(|r| job.release_sort != v::ReleaseSort::Added || !brought.contains(&r.release.id))
        .map(|r| res.release(r))
        .collect();
    let artists: Vec<_> = artists
        .iter()
        .map(|a| {
            let mut card = res.artist(a);
            card.track_count = per_artist.get(&a.artist.id).copied().unwrap_or(card.track_count);
            card
        })
        .collect();
    let recent: Vec<_> = recent.iter().map(|r| res.release(r)).collect();
    let often: Vec<_> = often.iter().map(|r| res.release(r)).collect();
    // The first rows of «Треки», as `Worker::load_tracks` reads them.
    let track_key = match job.track_sort {
        v::TrackSort::Artist => Sort::asc(TrackSortKey::Artist),
        v::TrackSort::Title => Sort::asc(TrackSortKey::Title),
        v::TrackSort::Album => Sort::asc(TrackSortKey::Album),
        v::TrackSort::Added => Sort::desc(TrackSortKey::Added),
        v::TrackSort::Duration => Sort::asc(TrackSortKey::Duration),
    };
    let rows = kept(
        &mut errors,
        "треки",
        lib.tracks_filtered(0, FIRST_TRACKS as u32, track_key, crate::track_filter(only_local)),
    );
    let track_rows = rows.iter().map(|r| res.track(r)).collect();
    let ms = started.elapsed().as_millis();
    if ms >= 250 {
        log::info!(
            "library: {} albums, {} artists read in {ms} ms off the loop",
            release_cards.len(),
            artists.len()
        );
    }
    Loaded {
        seq: job.seq,
        tracks_asked: job.tracks_asked,
        releases: release_cards,
        playlists,
        liked_albums,
        artists,
        recent,
        often,
        count,
        tracks: track_rows,
        track_sort: job.track_sort,
        errors,
    }
}

/// Rows of «Треки» a load reads (the list asks for more as it scrolls).
const FIRST_TRACKS: usize = 200;

impl Worker {
    /// `Command::LoadLibrary` and every reload: what the library feeds is
    /// told at once, the pages are read off the loop and shown together
    /// ([`Self::library_loaded`]).
    pub(crate) fn load_library(&mut self) {
        self.storage_changed();
        self.home_input_changed();
        self.stream_analysis_changed();
        self.recs_library_changed();
        self.publish_recordings();
        self.turn.lap("recordings");
        self.library_load.seq += 1;
        let job = {
            let s = self.state.read();
            Job {
                seq: self.library_load.seq,
                tracks_asked: self.library_load.tracks_asked,
                release_sort: s.library.release_sort,
                track_sort: s.library.track_sort,
                only_local: s.prefs.library_only_downloaded,
            }
        };
        let job = match &self.library_load.jobs {
            Some(jobs) => match jobs.send(job) {
                Ok(()) => return,
                Err(e) => e.0,
            },
            None => job,
        };
        if self.library_load.jobs.take().is_some() {
            log::warn!("library: its loading thread stopped, the library is read on the loop");
        }
        let loaded = read(&mut self.res, &job);
        self.turn.lap("read");
        self.library_loaded(loaded);
    }

    /// A load is on its way: an edit of the library asks for a new one
    /// rather than patching rows its answer would put back.
    pub(crate) fn library_load_pending(&self) -> bool {
        self.library_load.shown < self.library_load.seq
    }

    /// The pages of load `l.seq` came: shown in one publish when it is the
    /// newest load asked.
    pub(crate) fn library_loaded(&mut self, l: Loaded) {
        if l.seq != self.library_load.seq {
            log::debug!("library: load {} came after a newer one was asked", l.seq);
            return;
        }
        self.library_load.shown = l.seq;
        for what in &l.errors {
            self.toast(format!("Ошибка: {what}"), None);
        }
        // Rows of «Треки» asked since this load was: they are newer.
        let tracks_fresh = l.tracks_asked == self.library_load.tracks_asked;
        let Loaded { releases, playlists, liked_albums, mut artists, recent, often, count, tracks, track_sort, .. } = l;
        // A photo saved while the cards were made.
        for a in artists.iter_mut().filter(|a| a.image.is_none()) {
            a.image = self.res.photo(a.id);
        }
        self.publish(|s| {
            s.library.releases = releases;
            s.library.playlists = playlists;
            s.library.liked_albums = liked_albums;
            s.library.artists = artists;
            s.library.recently_added = recent;
            s.library.often_played = often;
            s.library.track_count = count;
            if tracks_fresh {
                s.library.tracks_offset = 0;
                s.library.tracks = tracks;
                s.library.track_sort = track_sort;
            }
            s.library.loaded = true;
        });
        self.turn.lap("shown");
    }
}
