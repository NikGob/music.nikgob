//! `NIKGOB_SHOT_STEPS` `fake:<what>`: states the demo cannot reach by
//! itself, laid over the runtime's view state for screenshots only (the
//! runtime never sees them).
//!
//! - `fake:discography` — the open library artist gets a «Нет в библиотеке»
//!   section from the fake Spotify catalog.
//! - `fake:wishlist` — a wishlist with every status.
//! - `fake:buffering` — the player waits for a stream.
//! - `fake:audio:youtube` / `fake:audio:spotify` — where the sound comes from.
//! - `fake:link` — the open playlist follows a Spotify playlist.

use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service};
use nikgob_view::{
    DiscographyView, PlaylistSyncView, Status, StreamAlbumCard, ViewState, WishlistGroup, WishlistId,
    WishlistOrigin, WishlistRow, WishlistScope, WishlistStatus as St, WishlistView,
};

#[derive(Clone, Debug, Default)]
pub struct Fakes {
    discography: bool,
    wishlist: bool,
    buffering: bool,
    audio: Option<Service>,
    link: bool,
}

impl Fakes {
    /// Takes `arg` of a `fake:` step; false when it is unknown.
    pub fn add(&mut self, arg: &str) -> bool {
        match arg {
            "discography" => self.discography = true,
            "wishlist" => self.wishlist = true,
            "buffering" => self.buffering = true,
            "audio:youtube" => self.audio = Some(Service::Youtube),
            "audio:spotify" => self.audio = Some(Service::Spotify),
            "link" => self.link = true,
            _ => return false,
        }
        true
    }

    pub fn any(&self) -> bool {
        self.discography || self.wishlist || self.buffering || self.audio.is_some() || self.link
    }

    /// `state` with the fakes laid over it.
    pub fn apply(&self, state: &ViewState) -> ViewState {
        let mut s = state.clone();
        if self.discography
            && let Some(a) = s.artist.as_mut()
        {
            a.discography = discography(&a.card.name, &state.library.releases);
        }
        if self.wishlist {
            s.downloads.wishlist = wishlist();
        }
        if self.buffering {
            s.player.status = Status::Buffering;
        }
        if let Some(svc) = self.audio {
            s.player.audio_service = Some(svc);
        }
        if self.link
            && let Some(p) = s.playlist.as_mut()
        {
            p.sync = Some(PlaylistSyncView {
                remote: RemoteId::new(Service::Spotify, "sp-pl-evening"),
                mode: PlaylistSyncMode::Follow,
                synced: Some("5 мин назад".into()),
                syncing: false,
                error: None,
            });
        }
        s
    }
}

/// Albums of the fake catalog the library has no release of (by `artist`
/// when there are such, else anyone's).
fn discography(artist: &str, library: &[nikgob_view::ReleaseCard]) -> DiscographyView {
    let all = FakeService::spotify().albums();
    let missing: Vec<_> = all.iter().filter(|a| !library.iter().any(|r| r.title == a.title)).collect();
    let mine: Vec<_> = missing.iter().copied().filter(|a| a.artists.iter().any(|x| x.name == artist)).collect();
    // The fake catalog is small: other artists' albums stand in for a
    // discography the library already has.
    let pick = if mine.is_empty() { missing } else { mine };
    let albums = pick
        .into_iter()
        .map(|a| StreamAlbumCard {
            remote: a.id.clone(),
            title: a.title.clone(),
            artist: nikgob_core::streaming::join_names(&a.artists),
            artist_id: a.artists.first().and_then(|x| x.id.clone()),
            year: a.year,
            release_type: a.release_type,
            cover_url: a.cover_url.clone(),
            total_tracks: a.total_tracks,
            release: None,
            local_tracks: 0,
        })
        .collect();
    DiscographyView { loading: false, error: None, albums }
}

fn row(n: u128, title: &str, artist: &str, album: &str, status: St) -> WishlistRow {
    WishlistRow {
        id: WishlistId(nikgob_core::model::Uuid::from_u128(n)),
        title: title.into(),
        artist: artist.into(),
        album: Some(album.into()),
        track: None,
        release: None,
        artist_id: None,
        status,
        scope: WishlistScope::Track,
        origin: WishlistOrigin::Like,
        note: match status {
            St::NoTorrent => Some("Не нашёл раздачу".into()),
            St::Downloading => Some("FLAC · 14 сидов · 38%".into()),
            St::Confirm => Some("Длительность расходится на 40 с".into()),
            _ => None,
        },
        next_try: (status == St::NoTorrent).then(|| "следующий поиск через 6 дн.".into()),
        attempts: u32::from(status == St::NoTorrent),
        can_save_from_youtube: status == St::NoTorrent,
    }
}

/// A wishlist with every status, in two albums and a loose track.
fn wishlist() -> WishlistView {
    let items = vec![
        row(1, "Кукушка", "Кино", "Чёрный альбом", St::Waiting),
        row(2, "Красно-жёлтые дни", "Кино", "Чёрный альбом", St::Waiting),
        row(3, "Нам с тобой", "Кино", "Чёрный альбом", St::Searching),
        row(4, "Automatic", "宇多田ヒカル", "First Love", St::Downloading),
        row(5, "First Love", "宇多田ヒカル", "First Love", St::Confirm),
        row(6, "Movin' on without you", "宇多田ヒカル", "First Love", St::NoTorrent),
        row(7, "Hyperballad", "Björk", "Post", St::Done),
        row(8, "Jóga", "Björk", "Homogenic", St::NoTorrent),
        row(9, "Bachelorette", "Björk", "Homogenic", St::Dismissed),
    ];
    let group = |title: &str, subtitle: &str, ids: &[u128]| WishlistGroup {
        title: title.into(),
        subtitle: subtitle.into(),
        release: None,
        artist: None,
        items: ids.iter().map(|n| WishlistId(nikgob_core::model::Uuid::from_u128(*n))).collect(),
        waiting: ids
            .iter()
            .filter(|n| items.iter().any(|r| r.id.0 == nikgob_core::model::Uuid::from_u128(**n) && r.status == St::Waiting))
            .count() as u32,
    };
    let by_album = vec![
        group("Чёрный альбом", "Кино", &[1, 2, 3]),
        group("First Love", "宇多田ヒカル", &[4, 5, 6]),
        group("Homogenic", "Björk", &[8, 9]),
    ];
    let by_artist = vec![
        group("Кино", "", &[1, 2, 3]),
        group("宇多田ヒカル", "", &[4, 5, 6]),
        group("Björk", "", &[7, 8, 9]),
    ];
    let mut counts: Vec<(St, u32)> = Vec::new();
    for r in &items {
        match counts.iter_mut().find(|(s, _)| *s == r.status) {
            Some((_, n)) => *n += 1,
            None => counts.push((r.status, 1)),
        }
    }
    WishlistView { loaded: true, items, by_album, by_artist, counts }
}
