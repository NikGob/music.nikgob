//! Query-side types: sort specs, list rows, reports.

use nikgob_core::model::{
    Artist, Codec, PlaylistEntry, PlaylistId, Release, Source, SourceId, SourceKind, TrackId,
    UnixMs,
};
use serde::{Deserialize, Serialize};

/// A sort key plus direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<K> {
    pub key: K,
    pub desc: bool,
}

impl<K> Sort<K> {
    pub const fn asc(key: K) -> Self {
        Self { key, desc: false }
    }

    pub const fn desc(key: K) -> Self {
        Self { key, desc: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackSortKey {
    #[default]
    Title,
    /// Artist, then album, disc, track number.
    Artist,
    /// Album, then disc, track number.
    Album,
    Added,
    Duration,
    Quality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReleaseSortKey {
    /// Artist, then year, then title.
    #[default]
    Artist,
    /// Year (unknown years last), then artist, title.
    Year,
    Title,
    Added,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ArtistSortKey {
    #[default]
    Name,
    Added,
}

pub type TrackSort = Sort<TrackSortKey>;
pub type ReleaseSort = Sort<ReleaseSortKey>;
pub type ArtistSort = Sort<ArtistSortKey>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackFilter {
    /// Every track, including those without a playable file.
    #[default]
    All,
    /// Only tracks whose active source is a local file.
    LocalOnly,
    /// Tracks without a local file (stream-only or file trashed).
    NoLocalFile,
}

/// One line of a (virtualised) track list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackRow {
    pub id: TrackId,
    pub title: String,
    /// Credit rendered as "A feat. B".
    pub artist: String,
    pub album: Option<String>,
    pub release_id: Option<nikgob_core::model::ReleaseId>,
    pub cover_ref: Option<String>,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration_ms: Option<u32>,
    /// `quality_score` of the active source.
    pub quality: Option<i32>,
    pub codec: Option<Codec>,
    pub active_kind: Option<SourceKind>,
    /// The active source is a local file.
    pub has_local: bool,
    pub liked: bool,
    /// Every credited artist with the text between them (`artist` is its
    /// display).
    #[serde(default)]
    pub credit: nikgob_core::model::ArtistCredit,
    /// The video id of the active source when it is a YouTube one.
    #[serde(default)]
    pub youtube_id: Option<String>,
    /// What the spectrum check of the active source found (a "FLAC" made
    /// from an MP3); `None` until it is checked.
    #[serde(default)]
    pub check: Option<nikgob_core::quality::CheckSummary>,
    /// A library track (`track.in_library`); a stream song heard only
    /// is not.
    #[serde(default)]
    pub in_library: bool,
}

impl TrackRow {
    /// The first credited artist, when linked to an artist row.
    pub fn artist_id(&self) -> Option<nikgob_core::model::ArtistId> {
        self.credit.primary().and_then(|p| p.artist_id)
    }

    /// What plays is a YouTube music video, not the song (a liked clip
    /// whose song YouTube Music does not have).
    pub fn is_clip(&self) -> bool {
        self.active_kind == Some(SourceKind::Youtube)
            && self.quality == Some(nikgob_core::model::score::STREAM_YOUTUBE_CLIP)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRow {
    pub release: Release,
    /// Tracks of this release that have a local file.
    pub local_tracks: u32,
    /// Tracks of this release known to the library.
    pub known_tracks: u32,
    /// The album is liked: saved, or, while album likes like the tracks
    /// too, every (local) track of it is; see [`crate::Library::like_release`].
    pub liked: bool,
    /// Rows of its page ([`crate::Library::tracks_of_release_all`]: outside
    /// the library too).
    pub rows: u32,
    /// The user's songs of it: rows of its page that are library tracks and
    /// not left out of it by hand (`crate::selection`).
    #[serde(default)]
    pub wanted: u32,
    /// Of them, the ones with a file.
    #[serde(default)]
    pub wanted_local: u32,
    /// The whole album is the user's: added whole or saved (its own like).
    #[serde(default)]
    pub whole: bool,
    /// A row of its page was left out of the user's songs by hand.
    #[serde(default)]
    pub left_out: bool,
    /// What its page shows while only some of it is the user's:
    /// `Some(true)` the whole album, `Some(false)` the user's songs, `None`
    /// the setting ([`crate::Library::set_release_view`]).
    #[serde(default)]
    pub view: Option<bool>,
}

impl ReleaseRow {
    /// The album is the user's as an album: added to the library or saved
    /// by hand ([`Self::whole`]), or every place of a tracklist of known
    /// length, two or more, is one of the user's songs (the same positive
    /// evidence migration 0028 takes; an unknown count proves nothing).
    pub fn is_added(&self) -> bool {
        let p = self.progress();
        self.whole
            || (self.release.total_tracks.is_some() && p.total >= 2 && p.wanted >= p.total && !p.left_out)
    }

    /// "3 из 12 у тебя", as its page counts ([`ReleaseListing`]), and the
    /// user's songs of it.
    pub fn progress(&self) -> ReleaseProgress {
        ReleaseProgress::new(self.local_tracks, self.known_tracks, self.rows, self.release.total_tracks)
            .mine(self.wanted, self.wanted_local, self.whole, self.left_out)
    }
}

/// What a row of an album's page is to the user ([`ReleaseListing::membership`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Membership {
    /// One of the user's songs of the album.
    Wanted,
    /// Another version of one of the album's songs («(Instrumental)»,
    /// «(Off Vocal)», «(Remix)»…) the user did not choose.
    Variant,
    /// Left out of the user's songs of the album by hand.
    Excluded,
    /// Only in the album's tracklist: nothing says the user wants it or not.
    Undecided,
}

/// One line of an album's page ([`crate::Library::release_listing`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ListingEntry {
    Track(TrackRow),
    /// A place the album counts that no row has: `no` 0 when unknown.
    Hole { disc: u32, no: u32 },
}

/// An album's page: its rows and holes, and the counts of both; which rows
/// are the user's songs of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseListing {
    pub entries: Vec<ListingEntry>,
    pub progress: ReleaseProgress,
    /// The rows that are the user's songs of the album.
    pub wanted: std::collections::HashSet<TrackId>,
    /// The rows left out of them by hand.
    pub excluded: std::collections::HashSet<TrackId>,
    /// What its page shows while only some of it is the user's (see
    /// [`ReleaseRow::view`]).
    pub view: Option<bool>,
    /// The rows that are library tracks for a reason of their own (a file,
    /// a like, a playlist, added here), not only for the album's: the
    /// user's songs under «Только мои песни».
    pub own: std::collections::HashSet<TrackId>,
    /// The rows the user chose by hand (a like, a playlist, added here): a
    /// version of a song among them stays theirs on an album added whole.
    pub picked: std::collections::HashSet<TrackId>,
    /// Whether its other versions play with it: `Some(on)` for this album
    /// ([`crate::Library::set_release_versions`]), `None` the setting.
    pub versions: Option<bool>,
}

/// What the user set for how albums show and play (Настройки → Музыка →
/// «Альбомы»).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlbumRules {
    pub mode: nikgob_core::model::AlbumMode,
    /// «Другие версии (инструментал, ремиксы)»: the other versions of an
    /// album's songs the user did not choose play with it (karaoke only
    /// when the album itself says so).
    pub other_versions: bool,
}

impl AlbumRules {
    /// What to store for an album's «Другие версии» switch turned `on`:
    /// on is always stored (karaoke plays only by the album's own on);
    /// off is stored only against the setting.
    pub fn versions_choice(&self, on: bool) -> Option<bool> {
        if on {
            Some(true)
        } else if self.other_versions {
            Some(false)
        } else {
            None
        }
    }
}

/// What an album's page shows and «Слушать» plays, worked out once for
/// every place that asks ([`ReleaseListing::plan`]): its page, ▶ of its
/// cards and menus, the queue its tracklist joins, Главная, search.
#[derive(Clone, Debug, PartialEq)]
pub struct AlbumPlan {
    /// Only some of it is the user's: its page has «Мои треки» and «Весь
    /// альбом».
    pub split: bool,
    /// Of such an album: its page opens on «Весь альбом» and «Слушать»
    /// plays it («Всегда весь альбом»).
    pub opens_whole: bool,
    /// What each row is to the user.
    pub membership: std::collections::HashMap<TrackId, Membership>,
    /// The user's songs, in the album's order («Мои треки»).
    pub mine: Vec<TrackId>,
    /// What the whole album plays, in its order: every row but the other
    /// versions it does not play (and, of an album not split, the songs
    /// left out of it).
    pub whole: Vec<TrackId>,
    /// The other versions of its songs the user did not choose, with their
    /// kind, in its order.
    pub versions: Vec<(TrackId, nikgob_core::clip_title::VersionKind)>,
    /// They play with the album (the album's own choice, else the
    /// setting): the switch of their group.
    pub versions_on: bool,
}

impl ReleaseListing {
    /// `unwanted`: the places of songs the user deleted from the album
    /// (`crate::unwanted`); within its count and free, they are no holes.
    pub(crate) fn new(rows: Vec<TrackRow>, total_tracks: Option<u32>, unwanted: &[(u32, u32)]) -> Self {
        let local = rows.iter().filter(|r| r.has_local).count() as u32;
        let count = total_tracks.unwrap_or(0);
        let placed: std::collections::HashSet<(u32, u32)> = rows.iter().map(|r| (r.disc_no.max(1), r.track_no)).collect();
        let gone: std::collections::HashSet<(u32, u32)> =
            unwanted.iter().copied().filter(|(d, n)| (1..=count).contains(n) && !placed.contains(&(*d, *n))).collect();
        let total_tracks = total_tracks.map(|t| t.saturating_sub(gone.len() as u32));
        let progress = ReleaseProgress::new(local, local, rows.len() as u32, total_tracks);
        let holes = progress.total as usize - rows.len();
        let discs: std::collections::BTreeSet<u32> = rows.iter().map(|r| r.disc_no.max(1)).collect();
        let disc = discs.iter().next_back().copied().unwrap_or(1);
        // Numbered holes on one disc: the free numbers up to the count.
        let mut numbered: Vec<u32> = Vec::new();
        if discs.len() <= 1 {
            let taken: std::collections::HashSet<u32> = rows.iter().map(|r| r.track_no).collect();
            let skipped: std::collections::HashSet<u32> = gone.iter().map(|(_, n)| *n).collect();
            numbered = (1..=progress.total.max(count))
                .filter(|n| !taken.contains(n) && !skipped.contains(n))
                .take(holes)
                .collect();
        }
        let mut entries = Vec::with_capacity(progress.total as usize);
        let mut next = numbered.iter().copied().peekable();
        for r in rows {
            while let Some(&n) = next.peek() {
                if r.track_no == 0 || n > r.track_no {
                    break;
                }
                entries.push(ListingEntry::Hole { disc, no: n });
                next.next();
            }
            entries.push(ListingEntry::Track(r));
        }
        entries.extend(next.map(|no| ListingEntry::Hole { disc, no }));
        while entries.len() < progress.total as usize {
            entries.push(ListingEntry::Hole { disc, no: 0 });
        }
        Self {
            entries,
            progress,
            wanted: std::collections::HashSet::new(),
            excluded: std::collections::HashSet::new(),
            view: None,
            own: std::collections::HashSet::new(),
            picked: std::collections::HashSet::new(),
            versions: None,
        }
    }

    /// The rows with a reason of their own to be the user's, the ones they
    /// chose by hand, and the album's own choice of its other versions.
    pub(crate) fn with_own(
        mut self,
        own: std::collections::HashSet<TrackId>,
        picked: std::collections::HashSet<TrackId>,
        versions: Option<bool>,
    ) -> Self {
        self.own = own;
        self.picked = picked;
        self.versions = versions;
        self
    }

    /// The user's songs of the album (`wanted`, a subset of its rows), the
    /// rows left out by hand, whether the whole album is theirs and what
    /// its page shows: counted into [`Self::progress`].
    pub(crate) fn with_choices(
        mut self,
        wanted: std::collections::HashSet<TrackId>,
        excluded: std::collections::HashSet<TrackId>,
        whole: bool,
        view: Option<bool>,
    ) -> Self {
        let local = self.tracks().filter(|t| t.has_local && wanted.contains(&t.id)).count() as u32;
        self.progress = self.progress.mine(wanted.len() as u32, local, whole, !excluded.is_empty());
        self.wanted = wanted;
        self.excluded = excluded;
        self.view = view;
        self
    }

    /// The user's songs of the album, in its order.
    pub fn wanted_ids(&self) -> Vec<TrackId> {
        self.tracks().filter(|t| self.wanted.contains(&t.id)).map(|t| t.id).collect()
    }

    /// Every row of the album, in its order.
    pub fn track_ids(&self) -> Vec<TrackId> {
        self.tracks().map(|t| t.id).collect()
    }

    /// The rows that are another version of a song of the same album: an
    /// instrumental, off vocal, karaoke or remix ([`version_mark`]) whose
    /// title without the mark is the title of a plain row of the album, the
    /// artists agreeing («SugaRiddle (Instrumental)» beside «SugaRiddle»).
    /// A mark without its song beside it proves nothing: a remix album, an
    /// instrumental piece of an OST, a single of only the karaoke.
    ///
    /// [`version_mark`]: nikgob_core::clip_title::version_mark
    pub fn version_kinds(&self) -> std::collections::HashMap<TrackId, nikgob_core::clip_title::VersionKind> {
        use nikgob_core::clip_title::{fold, version_mark};
        let marks: Vec<(&TrackRow, Option<(String, nikgob_core::clip_title::VersionKind)>)> =
            self.tracks().map(|t| (t, version_mark(&t.title))).collect();
        let plain: Vec<(String, String)> =
            marks.iter().filter(|(_, m)| m.is_none()).map(|(t, _)| (fold(&t.title), fold(&t.artist))).collect();
        let agree = |a: &str, b: &str| a.is_empty() || b.is_empty() || a.contains(b) || b.contains(a);
        marks
            .into_iter()
            .filter_map(|(t, m)| {
                let (base, kind) = m?;
                let (base, artist) = (fold(&base), fold(&t.artist));
                plain.iter().any(|(title, a)| *title == base && agree(a, &artist)).then_some((t.id, kind))
            })
            .collect()
    }

    /// What the album's page shows and «Слушать» plays under `rules`: the
    /// one place that says which songs of an album are the user's.
    ///
    /// * The user's songs: under «Автоматически» and «Всегда весь альбом»
    ///   its rows that are library tracks ([`Self::wanted`]: of an album
    ///   added to the library, all of them); under «Только мои песни» the
    ///   rows with a reason of their own ([`Self::own`]: a file, a like, a
    ///   playlist, added here), whether the album was added or not. Never
    ///   the rows left out by hand.
    /// * Split (two views on its page): only some of its places are the
    ///   user's songs. An album added to the library is whole, unless
    ///   «Только мои песни» says otherwise.
    /// * Another version of a song ([`Self::version_kinds`]) the user did
    ///   not choose is a [`Membership::Variant`]: on an album added whole
    ///   (or whole by having every song) only a version with a reason of
    ///   its own ([`Self::own`]: a file, a like, a playlist, put in by
    ///   hand) is theirs; on another album any of their songs is. Variants play with the album only when it says so (its
    ///   own choice, else [`AlbumRules::other_versions`], never karaoke by
    ///   the setting alone).
    pub fn plan(&self, rules: AlbumRules) -> AlbumPlan {
        use nikgob_core::clip_title::VersionKind;
        use nikgob_core::model::AlbumMode;
        let kinds = self.version_kinds();
        let p = self.progress;
        let added = p.whole || !p.partial();
        let mine_set = |id: &TrackId| {
            !self.excluded.contains(id)
                && match rules.mode {
                    AlbumMode::Mine => self.own.contains(id),
                    AlbumMode::Auto | AlbumMode::Whole => self.wanted.contains(id),
                }
        };
        let split = match rules.mode {
            AlbumMode::Auto | AlbumMode::Whole => p.partial(),
            // An album added whole with none of its songs the user's own is
            // an empty «Мои треки», not a page of the catalog.
            AlbumMode::Mine => {
                let n = self.tracks().filter(|t| mine_set(&t.id)).count() as u32;
                (n > 0 || p.left_out || p.whole) && n < p.total
            }
        };
        let versions_on = self.versions.unwrap_or(rules.other_versions);
        let plays_kind = |kind: VersionKind| match self.versions {
            Some(on) => on,
            None => rules.other_versions && kind != VersionKind::Karaoke,
        };
        let mut membership = std::collections::HashMap::new();
        let (mut mine, mut whole, mut versions) = (Vec::new(), Vec::new(), Vec::new());
        for t in self.tracks() {
            let kind = kinds.get(&t.id).copied();
            // A version the user owns for a reason of its own (a file, a
            // like, a playlist, put in by hand) stays theirs on an album
            // added whole too; only the ones that came with the album wait
            // for the switch.
            let kept = mine_set(&t.id) && (!added || kind.is_none() || self.own.contains(&t.id));
            let m = if self.excluded.contains(&t.id) {
                Membership::Excluded
            } else if kept {
                Membership::Wanted
            } else if kind.is_some() {
                Membership::Variant
            } else {
                Membership::Undecided
            };
            membership.insert(t.id, m);
            if m == Membership::Wanted {
                mine.push(t.id);
            }
            let plays = match (m, kind) {
                (Membership::Variant, Some(k)) => plays_kind(k),
                (Membership::Excluded, _) => split,
                _ => true,
            };
            if plays {
                whole.push(t.id);
            }
            if let (Membership::Variant, Some(k)) = (m, kind) {
                versions.push((t.id, k));
            }
        }
        AlbumPlan {
            split,
            opens_whole: rules.mode == AlbumMode::Whole,
            membership,
            mine,
            whole,
            versions,
            versions_on,
        }
    }

    /// What «Слушать» of the album plays under `rules`, in its order, and
    /// whether that is the whole album (its tracklist found later joins
    /// such a queue whole; else only new songs of the user's). `scope`:
    /// `Some(true)` the whole album («Слушать весь альбом»), `Some(false)`
    /// the user's songs, `None` as its page opens. A split album plays the
    /// user's songs (none: nothing); one not split plays as it is the
    /// user's ([`AlbumPlan::whole`]). A song picked outside what plays
    /// (`start`) plays the whole album from it, that song too.
    pub fn plays(&self, rules: AlbumRules, scope: Option<bool>, start: Option<TrackId>) -> (Vec<TrackId>, bool) {
        let plan = self.plan(rules);
        let whole = scope.unwrap_or(plan.split && plan.opens_whole);
        let (list, whole) = match (plan.split, whole) {
            (_, true) => (plan.whole.clone(), true),
            (true, false) => (plan.mine.clone(), false),
            // None of it the user's: a page of the catalog, played whole.
            (false, false) => (plan.whole.clone(), plan.mine.is_empty()),
        };
        match start {
            Some(s) if !list.contains(&s) && self.tracks().any(|t| t.id == s) => {
                (self.tracks().filter(|t| t.id == s || plan.whole.contains(&t.id)).map(|t| t.id).collect(), true)
            }
            _ => (list, whole),
        }
    }

    /// The rows of the album a queue of it takes in when its tracklist
    /// grows: of a queue of the whole album what the whole album plays,
    /// else the user's songs ([`Self::plays`]).
    pub fn joins(&self, rules: AlbumRules, whole: bool) -> Vec<TrackId> {
        let plan = self.plan(rules);
        if whole { plan.whole } else { plan.mine }
    }

    pub fn holes(&self) -> usize {
        self.entries.iter().filter(|e| matches!(e, ListingEntry::Hole { .. })).count()
    }

    pub fn tracks(&self) -> impl Iterator<Item = &TrackRow> {
        self.entries.iter().filter_map(|e| match e {
            ListingEntry::Track(t) => Some(t),
            ListingEntry::Hole { .. } => None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseProgress {
    /// Tracks with a local file ("3 из 12 у тебя").
    pub local: u32,
    /// Tracks in the library: with a file, or liked / in a playlist from a
    /// service (stream only, "стрим").
    #[serde(default)]
    pub in_library: u32,
    /// `total_tracks` of the release, or the rows of its page if more or
    /// unknown: the lines of its page ([`ReleaseListing`]).
    pub total: u32,
    /// The user's songs of it ([`ReleaseRow::wanted`]).
    #[serde(default)]
    pub wanted: u32,
    /// Of them, the ones with a file.
    #[serde(default)]
    pub wanted_local: u32,
    /// The whole album is the user's ([`ReleaseRow::whole`]).
    #[serde(default)]
    pub whole: bool,
    /// A row of it was left out of the user's songs by hand
    /// ([`ReleaseRow::left_out`]).
    #[serde(default)]
    pub left_out: bool,
}

impl ReleaseProgress {
    /// The counts of the album; its tracks in the library stand for the
    /// user's songs until [`Self::mine`] says.
    pub(crate) fn new(local: u32, known: u32, rows: u32, total_tracks: Option<u32>) -> Self {
        Self {
            local,
            in_library: known,
            total: total_tracks.unwrap_or(0).max(rows).max(known),
            wanted: known,
            wanted_local: local.min(known),
            whole: false,
            left_out: false,
        }
    }

    /// With the user's songs of the album counted.
    pub(crate) fn mine(self, wanted: u32, wanted_local: u32, whole: bool, left_out: bool) -> Self {
        Self { wanted, wanted_local, whole, left_out, ..self }
    }

    /// Lines of the page without a file: «1 нет».
    pub fn missing(&self) -> u32 {
        self.total.saturating_sub(self.local)
    }

    /// Only some of the album is the user's: some of its songs, not all
    /// of its places (a place without a row counts), not added whole.
    /// Its page shows and plays the user's songs, nothing of the rest is
    /// owed; an album with none of them is a plain catalog page, unless
    /// the user left songs of it out by hand (then it stays theirs to
    /// choose from, with none of its songs to play or download).
    pub fn partial(&self) -> bool {
        !self.whole && (self.wanted > 0 || self.left_out) && self.wanted < self.total
    }

    /// The user's songs of the album without a file.
    pub fn wanted_missing(&self) -> u32 {
        self.wanted.saturating_sub(self.wanted_local)
    }

    /// What «Скачать недостающие» asks for: of a partial album the user's
    /// songs without a file, else every place without one.
    pub fn lacking(&self) -> u32 {
        if self.partial() { self.wanted_missing() } else { self.missing() }
    }

    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.local >= self.total
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRow {
    pub artist: Artist,
    pub release_count: u32,
    /// Credited tracks that have a local file.
    pub local_track_count: u32,
    /// The artist itself is liked.
    #[serde(default)]
    pub liked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<TrackRow>,
    pub releases: Vec<ReleaseRow>,
    pub artists: Vec<ArtistRow>,
    pub playlists: Vec<PlaylistSummary>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.releases.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistKind {
    /// The pinned virtual "Любимые" playlist (liked tracks).
    Liked,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    /// `None` for the virtual liked playlist.
    pub id: Option<PlaylistId>,
    pub kind: PlaylistKind,
    pub name: String,
    pub cover: Option<String>,
    pub track_count: u32,
    pub updated_at: UnixMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistEntryRow {
    pub entry: PlaylistEntry,
    pub track: TrackRow,
}

/// Aggregates over the event log, for recommendations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackStats {
    /// Number of `start` events.
    pub plays: u32,
    pub completes: u32,
    pub skips: u32,
    /// Latest `start` or `complete`.
    pub last_played: Option<UnixMs>,
}

/// A source sitting in Trash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
    pub source: Source,
    /// Where the import crate moved the file inside `Trash/`, if it told us.
    pub trash_path: Option<String>,
    pub trashed_at: UnixMs,
    pub track_title: String,
    pub artist: String,
}

/// A track the user deleted ("Корзина").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedTrack {
    pub track: TrackRow,
    pub deleted_at: UnixMs,
}

/// What [`crate::Library::purge_track`] removed; the caller deletes the files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgedTrack {
    /// Files of trashed sources, inside `Trash/` (or their original place when
    /// the file was already gone at trashing time and nothing lives there now).
    pub trash_files: Vec<String>,
    /// Files of live local sources (a permanent delete straight from the library).
    pub live_files: Vec<String>,
    /// Where every local file of the track lived in the library (trashed ones
    /// too): the album folders to tidy up.
    pub places: Vec<String>,
    /// Content hash of the lossless master, when the track had one: its file
    /// and sidecar in `.musiclib/masters/` go too.
    pub master_hash: Option<String>,
    /// The release went with its last track.
    pub release_removed: bool,
}

/// A file parked in «Подтверди» (see `Library::put_confirm_item`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmRow {
    pub id: i64,
    pub path: String,
    /// The «Хочу скачать» item the file was downloaded for.
    pub wishlist: Option<nikgob_core::model::WishlistId>,
    /// The importer's own json.
    pub payload: String,
    pub created_at: UnixMs,
}

/// One finished job of "Загрузки → История".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// Row id, increasing.
    pub seq: i64,
    /// `import`, `torrent`, or `wish:<item>` (the one line of a wished
    /// download).
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub error: Option<String>,
    pub at: UnixMs,
}

/// What [`crate::Library::purge_trash`] removed from the DB; the caller deletes the files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgedFile {
    pub source_id: SourceId,
    pub track_id: TrackId,
    /// File to delete from disk: the recorded `trash_path`, else (for local files)
    /// the original locator. `None` for stream sources, which have no file.
    pub path: Option<String>,
    pub kind: SourceKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    pub playlists: usize,
    pub playlist_entries: usize,
    pub likes: usize,
    pub events: usize,
    /// Stream-only tracks written to `streams.json` (liked or in playlists).
    pub stream_tracks: usize,
    /// «Не рекомендовать» marks written to `rec_feedback.json`.
    pub rec_feedback: usize,
    /// Marks of songs saved in a service's library written to `saved.json`.
    pub saved: usize,
    /// Files whose content actually changed on disk.
    pub files_written: usize,
}

/// A library playlist linked to a playlist in a streaming service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistRemote {
    pub playlist_id: nikgob_core::model::PlaylistId,
    pub remote: nikgob_core::streaming::RemoteId,
    pub sync_mode: nikgob_core::streaming::PlaylistSyncMode,
    /// The service's snapshot id / etag at the last sync.
    pub remote_version: Option<String>,
    pub synced_at: Option<nikgob_core::model::UnixMs>,
    /// The content at the last sync (JSON, shape owned by the sync code):
    /// the base of a three-way merge.
    pub base_json: Option<String>,
}

/// One liked track of a streaming service as the last pull saw it (table
/// `remote_like`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteLike {
    pub remote: nikgob_core::streaming::RemoteId,
    pub track_id: TrackId,
    /// The local like was made by a pull from this service (so the service
    /// may take it back); `false` = the user liked it here (or before).
    pub pulled: bool,
    pub seen_at: UnixMs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Rows inserted or overwritten because the incoming copy won.
    pub applied: usize,
    /// Rows ignored because the local copy was newer (or identical).
    pub skipped: usize,
    pub events_added: usize,
    /// Stream tracks recreated (or re-linked) from `streams.json`.
    pub stream_tracks: usize,
    /// Marks of songs saved in a service's library taken from `saved.json`.
    pub saved: usize,
    /// Unparsable lines in `events.jsonl`.
    pub invalid_lines: usize,
}

/// A track whose local file is due for sound analysis
/// ([`crate::Library::tracks_needing_analysis`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisTodo {
    pub track_id: TrackId,
    /// The active local source.
    pub source_id: SourceId,
    /// Its path (relative to the library root, or absolute).
    pub locator: String,
    /// Which file this is (source id, hash, size): a failure is remembered
    /// for this key ([`crate::Library::mark_analysis_failed`]).
    pub file_key: String,
}

/// A stream-only track due for sound analysis
/// ([`crate::Library::streams_needing_analysis`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamAnalysisTodo {
    pub track_id: TrackId,
    /// Its YouTube video, if it has one that plays (else its song has to
    /// be found on YouTube Music first).
    pub youtube: Option<String>,
}

/// How far the sound analysis is ([`crate::Library::analysis_counts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnalysisCounts {
    /// Tracks with a file analysed from it with the current analysis.
    pub files_done: u32,
    /// Tracks with a file, without those that failed to decode.
    pub files_total: u32,
    /// Stream-only tracks that are liked or in a playlist and have a
    /// current analysis.
    pub streams_done: u32,
    pub streams_total: u32,
}

/// One «Не рекомендовать» mark ([`crate::Library::set_rec_feedback`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecFeedback {
    pub target_kind: String,
    pub target_id: String,
    pub dislike: bool,
    pub updated_at: UnixMs,
}
