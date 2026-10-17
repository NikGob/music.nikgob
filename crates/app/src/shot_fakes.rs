//! `NIKGOB_SHOT_STEPS` `fake:<what>`: states the demo cannot reach by
//! itself, laid over the runtime's view state for screenshots only (the
//! runtime never sees them).
//!
//! - `fake:discography` — the open library artist gets a «Нет в библиотеке»
//!   section from the fake Spotify catalog.
//! - `fake:wishlist` — a wishlist with every status.
//! - `fake:album-status` — the first stream album cards on screen (the
//!   discography, a service's artist page, search) are queued, downloading,
//!   fully local and partly local, in that order.
//! - `fake:buffering` — the player waits for a stream.
//! - `fake:audio:youtube` / `fake:audio:spotify` — where the sound comes from.
//! - `fake:link` — the open playlist follows a Spotify playlist.
//! - `fake:lyrics` — word-synced lyrics (LyricsPlus) for whatever plays:
//!   the demo has no network to fetch any for a stream track.
//! - `fake:paused:youtube` / `fake:paused:spotify` — the service asked to
//!   wait (until 14:05).
//! - `fake:torrents` — Prowlarr looks set up (the demo has none).
//! - `fake:no-sound` — Spotify signed in with Premium, the sound consent
//!   not given yet.

use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service};
use nikgob_view::{
    DiscographyView, PlaylistSyncView, Status, StreamAlbumCard, ViewState, WishlistGroup, WishlistId,
    WishlistOrigin, WishlistRow, WishlistScope, WishlistStatus as St, WishlistView,
};

#[derive(Clone, Debug, Default)]
pub struct Fakes {
    discography: bool,
    wishlist: bool,
    album_status: bool,
    buffering: bool,
    audio: Option<Service>,
    link: bool,
    lyrics: bool,
    paused: Option<Service>,
    torrents: bool,
    no_sound: bool,
}

impl Fakes {
    /// Takes `arg` of a `fake:` step; false when it is unknown.
    pub fn add(&mut self, arg: &str) -> bool {
        match arg {
            "discography" => self.discography = true,
            "wishlist" => self.wishlist = true,
            "album-status" => self.album_status = true,
            "buffering" => self.buffering = true,
            "audio:youtube" => self.audio = Some(Service::Youtube),
            "audio:spotify" => self.audio = Some(Service::Spotify),
            "link" => self.link = true,
            "lyrics" => self.lyrics = true,
            "paused:youtube" => self.paused = Some(Service::Youtube),
            "paused:spotify" => self.paused = Some(Service::Spotify),
            "torrents" => self.torrents = true,
            "no-sound" => self.no_sound = true,
            _ => return false,
        }
        true
    }

    pub fn any(&self) -> bool {
        self.discography
            || self.wishlist
            || self.album_status
            || self.buffering
            || self.audio.is_some()
            || self.link
            || self.lyrics
            || self.paused.is_some()
            || self.torrents
            || self.no_sound
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
        if self.album_status {
            let mut rows = Vec::new();
            if let Some(a) = s.artist.as_mut() {
                album_statuses(&mut a.discography.albums, &mut rows);
            }
            if let Some(p) = s.stream_page.as_mut() {
                album_statuses(&mut p.albums, &mut rows);
            }
            for x in &mut s.search.streams {
                album_statuses(&mut x.albums, &mut rows);
            }
            s.downloads.wishlist.items.extend(rows);
        }
        if self.buffering {
            s.player.status = Status::Buffering;
        }
        if let Some(svc) = self.paused
            && let Some(a) = s.accounts.iter_mut().find(|a| a.service == svc)
        {
            a.paused_until = Some(0);
            a.paused_until_hm = Some("14:05".into());
        }
        if self.torrents {
            s.prefs.prowlarr_configured = true;
        }
        if self.no_sound
            && let Some(a) = s.accounts.iter_mut().find(|a| a.service == Service::Spotify)
        {
            a.audio = false;
            if let nikgob_view::Account::LoggedIn { premium, .. } = &mut a.account {
                *premium = Some(true);
            }
        }
        if let Some(svc) = self.audio {
            s.player.audio_service = Some(svc);
            // What that service's sound is.
            s.player.audio_quality = Some(match svc {
                Service::Youtube => nikgob_core::streaming::quality_label("Opus", 160, true),
                Service::Spotify => nikgob_core::streaming::quality_label("Vorbis", 320, false),
            });
        }
        if self.lyrics {
            s.lyrics = word_lyrics(state.player.current.as_ref().map(|t| t.id));
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

/// Lines of three words, one word a second, a line every four seconds.
fn word_lyrics(track: Option<nikgob_view::TrackId>) -> nikgob_view::LyricsView {
    use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource};
    use std::time::Duration;
    let text = [
        "Тёплое место на улице ждёт",
        "Группа крови на рукаве",
        "Мой порядковый номер",
        "Пожелай мне удачи в бою",
        "Не остаться в этой траве",
        "Пожелай мне удачи",
    ];
    let ms = Duration::from_millis;
    let lines = (0..40)
        .map(|i| {
            let line = text[i % text.len()];
            let start = 1500 + 4000 * i as u64;
            let words: Vec<&str> = line.split(' ').collect();
            let step = 3200 / words.len() as u64;
            LyricLine {
                start: ms(start),
                end: Some(ms(start + 3400)),
                text: line.to_owned(),
                words: words
                    .iter()
                    .enumerate()
                    .map(|(j, w)| LyricWord {
                        start: ms(start + step * j as u64),
                        end: ms(start + step * (j as u64 + 1)),
                        text: if j + 1 < words.len() { format!("{w} ") } else { (*w).to_owned() },
                    })
                    .collect(),
                ..Default::default()
            }
        })
        .collect();
    nikgob_view::LyricsView {
        track,
        lines,
        plain: None,
        loading: false,
        source: Some(LyricsSource::LyricsPlus),
        picked: false,
        sources: Vec::new(),
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
    let mut pick = if mine.is_empty() { missing } else { mine };
    // Too few for a grid: the albums the library has too stand in.
    for a in &all {
        if pick.len() >= 6 {
            break;
        }
        if !pick.iter().any(|x| x.id == a.id) {
            pick.push(a);
        }
    }
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
            have: Default::default(),
        })
        .collect();
    DiscographyView { loading: false, error: None, albums }
}

/// Queued, downloading (3 of 10 on disk), all on disk, 4 of 12 on disk.
fn album_statuses(albums: &mut [StreamAlbumCard], rows: &mut Vec<WishlistRow>) {
    use nikgob_view::AlbumHave;
    let n = rows.len() as u128;
    for (i, a) in albums.iter_mut().take(4).enumerate() {
        match i {
            0 | 1 => {
                let st = if i == 0 { St::New } else { St::Downloading };
                let mut r = row(1000 + n + i as u128, &a.title, &a.artist, &a.title, st);
                r.scope = WishlistScope::Album;
                rows.push(r);
                if i == 1 {
                    a.have = AlbumHave { local: 3, in_library: 10, total: 10 };
                }
            }
            2 => a.have = AlbumHave { local: 9, in_library: 9, total: 9 },
            _ => a.have = AlbumHave { local: 4, in_library: 4, total: 12 },
        }
    }
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
        next_try: (status == St::NoTorrent).then(|| "поищу снова через 6 дн.".into()),
        attempts: u32::from(status == St::NoTorrent),
        can_save_from_youtube: status == St::NoTorrent,
        cover: None,
        progress: (status == St::Downloading).then_some(38),
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
