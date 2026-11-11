//! The library's part of the search, off the player's loop.
//!
//! [`Command::Search`] read the library on the worker's thread, a quarter
//! of a second and more for a word of a letter or two, while no click or
//! playback event was answered. Now the query goes to a thread of its own
//! with a resolver of its own ([`crate::Resolver::fork`]); the answer comes
//! back to the loop, which shows it and then asks the services, as it did.
//!
//! * **Newest wins.** Queries that wait while one is read are one read: the
//!   newest.
//! * **Stale answers are dropped.** Every query (an emptied field too) has
//!   a number; an answer to any but the newest is never shown, so a slow
//!   read cannot put an older query's rows over the newer one's.
//!
//! [`Command::Search`]: nikgob_view::Command::Search

use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use nikgob_view as v;

use crate::convert::Resolver;
use crate::{Later, Worker};

/// Most of each kind the library's part shows.
const SHOWN: u32 = 20;

struct Query {
    seq: u64,
    /// As typed (the view shows it so).
    raw: String,
}

/// The library's answer to query `seq`.
pub(crate) struct Answer {
    seq: u64,
    raw: String,
    tracks: Vec<v::TrackRow>,
    releases: Vec<v::ReleaseCard>,
    artists: Vec<v::ArtistCard>,
    playlists: Vec<v::PlaylistCard>,
    /// The tracks found with their ISRCs: the services' sections match
    /// their songs to them.
    lib_tracks: Vec<(nikgob_library::TrackRow, Option<String>)>,
}

impl Answer {
    /// The answer of a search without the library («Источники» of the
    /// search leave it out).
    fn empty(seq: u64, raw: String) -> Self {
        Self {
            seq,
            raw,
            tracks: Vec::new(),
            releases: Vec::new(),
            artists: Vec::new(),
            playlists: Vec::new(),
            lib_tracks: Vec::new(),
        }
    }
}

/// The worker's end of the search thread.
pub(crate) struct LibrarySearch {
    /// `None`: no thread (it did not start or it stopped): read on the loop.
    queries: Option<Sender<Query>>,
    /// The newest query.
    seq: u64,
}

impl LibrarySearch {
    /// Starts the thread with a fork of the worker's resolver; answers go
    /// to the loop through `later`.
    pub(crate) fn start(res: &Resolver, later: Sender<Later>) -> Self {
        let (queries, inbox) = crossbeam_channel::unbounded::<Query>();
        let fork = res.fork();
        let spawned = std::thread::Builder::new().name("nikgob-search".into()).spawn(move || run(&inbox, &later, fork));
        let queries = match spawned {
            Ok(_) => Some(queries),
            Err(e) => {
                log::error!("search: its thread did not start, the library is read on the loop: {e}");
                None
            }
        };
        Self { queries, seq: 0 }
    }
}

fn run(inbox: &Receiver<Query>, later: &Sender<Later>, mut res: Resolver) {
    while let Ok(mut q) = inbox.recv() {
        while let Ok(newer) = inbox.try_recv() {
            q = newer;
        }
        let answer = read(&mut res, q.seq, q.raw);
        if later.send(Later::new("the library's search answer", move |w| w.library_answered(answer))).is_err() {
            return;
        }
    }
}

/// The library's answer to `raw` (its words, at most [`SHOWN`] of each kind).
fn read(res: &mut Resolver, seq: u64, raw: String) -> Answer {
    let started = Instant::now();
    let lib = res.library().clone();
    let text = raw.trim();
    let r = lib.search(text, SHOWN).unwrap_or_default();
    log::debug!(
        target: "nikgob_runtime::streams",
        "search «{text}»: library {} tracks, {} albums in {} ms",
        r.tracks.len(),
        r.releases.len(),
        started.elapsed().as_millis()
    );
    let tracks = r.tracks.iter().map(|t| res.track(t)).collect();
    let releases = r.releases.iter().map(|x| res.release(x)).collect();
    let artists = r.artists.iter().map(|a| res.artist(a)).collect();
    let playlists = r.playlists.iter().map(|p| res.playlist(p)).collect();
    let lib_tracks = r
        .tracks
        .into_iter()
        .map(|t| {
            let isrc = lib.get_track(t.id).ok().flatten().and_then(|x| x.isrc);
            (t, isrc)
        })
        .collect();
    Answer { seq, raw, tracks, releases, artists, playlists, lib_tracks }
}

impl Worker {
    /// `Command::Search`: the library's part is read off the loop, then
    /// shown with the services asked ([`Self::library_answered`]); an
    /// emptied field clears the search at once.
    pub(crate) fn search(&mut self, q: String) {
        self.library_search.seq += 1;
        let seq = self.library_search.seq;
        let sources = self.streaming.search.sources;
        if q.trim().is_empty() {
            self.search_streams("", Vec::new());
            self.publish(|s| {
                s.search = v::SearchView {
                    sources,
                    ..Default::default()
                };
            });
            return;
        }
        if !sources.library {
            self.library_answered(Answer::empty(seq, q));
            return;
        }
        let q = match &self.library_search.queries {
            Some(queries) => match queries.send(Query { seq, raw: q }) {
                Ok(()) => return,
                Err(e) => e.0.raw,
            },
            None => q,
        };
        log::warn!("search: its thread stopped, the library is read on the loop");
        self.library_search.queries = None;
        let answer = read(&mut self.res, seq, q);
        self.library_answered(answer);
    }

    /// The library answered query `a.seq`: shown when it is the newest
    /// (the services' sections and the merged answer stay until their new
    /// answers), then the services and the torrents are asked.
    pub(crate) fn library_answered(&mut self, a: Answer) {
        if a.seq != self.library_search.seq {
            log::debug!(target: "nikgob_runtime::streams", "search: the answer to «{}» came after a newer query", a.raw.trim());
            return;
        }
        let Answer { raw, tracks, releases, artists, playlists, lib_tracks, .. } = a;
        let trimmed = raw.trim().to_owned();
        let sources = self.streaming.search.sources;
        self.publish(|s| {
            s.search = v::SearchView {
                query: raw,
                tracks,
                releases,
                artists,
                playlists,
                searching: false,
                sources,
                // The services' sections and the merged answer stay until
                // their new answers.
                streams: std::mem::take(&mut s.search.streams),
                answer: std::mem::take(&mut s.search.answer),
                torrents: std::mem::take(&mut s.search.torrents),
            };
        });
        self.turn.lap("library shown");
        self.search_streams(&trimmed, lib_tracks);
        self.turn.lap("services asked");
        if sources.torrents && self.state.read().search.torrents.query != trimmed {
            self.search_torrents(trimmed);
        }
    }
}
