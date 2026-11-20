use std::collections::HashMap;
use std::sync::Arc;

use nikgob_view::{SpeedVariant, TrackMenuItem, TrackSpeeds};

use super::widgets::{Row, SubId, row_acts, row_items, shown_rows, sub_rows};
use super::*;

fn rows() -> Vec<Row<TrackAct>> {
    track_rows(TrackMenuOf {
        playable: true,
        has_album: true,
        has_artists: true,
        has_file: false,
        also_on: None,
        info: false,
        fetch: Some(MenuItem::new("Скачать")),
        better: false,
        pick_release: false,
        find_song: None,
        wrong_file: false,
        dislike: MenuItem::new("Не нравится"),
        ban: Some(MenuItem::new("Не рекомендовать исполнителя")),
        library: None,
    })
}

/// A library track with an album and an artist.
fn track(has_local: bool) -> TrackRow {
    TrackRow {
        id: TrackId::new(), title: "Track".into(), artist: "Artist".into(),
        album: Some("Album".into()), release_id: Some(ReleaseId::new()),
        artists: nikgob_view::Credit::single("Artist", Some(nikgob_view::ArtistId::new())), disc_no: 1, track_no: 1,
        duration: None, quality: None, source: None, has_local, liked: false, cover: None, clip: false, in_library: true, temporary: false,
    }
}

/// A library row's menu as a list has it, with no torrent source and
/// nothing in «Хочу скачать».
fn of(marks: &nikgob_view::DislikesView, info: bool) -> LibraryRowsOf<'_> {
    LibraryRowsOf { marks, info, also: None, torrents: false, wished: None }
}

fn speeds(enabled: bool, chosen: &[(TrackId, SpeedVariant)]) -> TrackSpeeds {
    TrackSpeeds { enabled, chosen: Arc::new(chosen.iter().copied().collect::<HashMap<_, _>>()) }
}

fn labels(rows: &[Row<TrackAct>]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Item(_, item) | Row::Sub(_, item, _) => Some(item.label.clone()),
            Row::Divider | Row::Label(_) => None,
        })
        .collect()
}

/// A submenu is found by its id, never by its translated label: the
/// player's «Файл» keeps its own items under either locale's label.
#[test]
fn the_file_submenu_is_found_by_id_under_any_label() {
    for label in [i18n::menus_in(i18n::Locale::Ru).file, i18n::menus_in(i18n::Locale::En).file] {
        let mut rows: Vec<Row<()>> = vec![Row::Sub(SubId::File, MenuItem::new(label), Vec::new())];
        assert!(widgets::sub_rows_mut(&mut rows, SubId::File).is_some(), "{label}");
        assert!(widgets::sub_rows_mut(&mut rows, SubId::Sleep).is_none(), "{label}");
    }
}

/// «Скорость…» is in the menu of the track playing, right before
/// «Файл», only while the speed of tracks is on in the settings; any
/// other row has none.
#[test]
fn the_speed_row_is_there_only_while_the_speed_of_tracks_is_on() {
    let track = TrackId::new();
    let plain = row_acts(&rows());
    let off = with_speed_row(rows(), &speeds(false, &[(track, SpeedVariant::Fast)]), true);
    assert_eq!(row_acts(&off), plain, "switched off: the menu as it was");
    let other = with_speed_row(rows(), &speeds(true, &[(track, SpeedVariant::Fast)]), false);
    assert_eq!(row_acts(&other), plain, "a track that does not play: no speed");

    let on = with_speed_row(rows(), &speeds(true, &[]), true);
    let acts = row_acts(&on);
    assert_eq!(acts.len(), plain.len() + 1);
    // «Скачать» of a track with no file, then the speed.
    let share = acts.iter().position(|a| *a == TrackAct::Share).unwrap();
    assert_eq!(acts[share + 1..share + 3], [TrackAct::Fetch, TrackAct::Speed]);
    let at = on.iter().position(|r| matches!(r, Row::Item(TrackAct::Speed, _))).unwrap();
    assert!(matches!(on[at - 1], Row::Item(TrackAct::Fetch, _)));
    assert!(matches!(&on[at + 1], Row::Sub(SubId::File, item, _) if item.label == i18n::menus_in(i18n::Locale::Ru).file));
}

/// The row is the same whatever the track's speed: nothing marks it.
#[test]
fn the_speed_row_is_not_marked() {
    let fast = TrackId::new();
    let s = speeds(true, &[(fast, SpeedVariant::Nightcore)]);
    let rows = with_speed_row(rows(), &s, true);
    let marked: Vec<bool> =
        row_items(&rows).into_iter().filter(|(a, _)| *a == TrackAct::Speed).map(|(_, item)| item.selected).collect();
    assert_eq!(marked, [false]);
}

/// The menu is in its groups: play next, the queue and the radio, then
/// «В плейлист…», then where the track is from; «Скачать» of a track
/// with no file in the menu itself, the rest of the files and the info
/// behind «Файл».
#[test]
fn the_menu_is_in_its_groups_and_the_files_are_in_a_submenu() {
    let shown = shown_rows(rows());
    let top = labels(&shown);
    let ru = i18n::menus_in(i18n::Locale::Ru);
    assert_eq!(&top[..4], [ru.play_next, ru.to_queue, ru.radio_by_track, ru.to_playlist]);
    assert!(matches!(shown[3], Row::Divider) && matches!(shown[5], Row::Divider));
    // «Файл» stays a submenu even with «Скачать альбом» alone.
    assert_eq!(&top[top.len() - 5..], [ru.share, ru.download, ru.file, ru.dislike, ru.ban_artist]);
    assert_eq!(row_acts(sub_rows(&shown, SubId::File).unwrap()), [TrackAct::AlbumDownload]);
    // The speed stands in the menu itself, right before «Файл».
    let top = labels(&with_speed_row(rows(), &speeds(true, &[]), true));
    let at = top.iter().position(|l| l == "Скорость…").unwrap();
    assert_eq!(top[at + 1], ru.file);
    let shown = shown_rows(with_speed_row(rows(), &speeds(true, &[]), true));
    // What is shown stands for the same actions in the same order.
    assert_eq!(row_acts(&shown), row_acts(&with_speed_row(rows(), &speeds(true, &[]), true)));
    // A library row's deletes close «Файл».
    let t = track(true);
    let marks = nikgob_view::DislikesView::default();
    let mut library = library_track_rows(&t, of(&marks, true));
    push_deletes(&mut library, &t);
    let file = row_acts(sub_rows(&library, SubId::File).unwrap());
    assert_eq!(
        file,
        [TrackAct::Info, TrackAct::Fetch, TrackAct::Better, TrackAct::WrongFile, TrackAct::RefreshCover, TrackAct::Trash, TrackAct::Purge]
    );
    let top = labels(&library);
    let share = top.iter().position(|l| l == "Поделиться").unwrap();
    assert_eq!(top[share + 1], ru.file);
    let acts = row_acts(&library);
    assert_eq!(&acts[acts.len() - 2..], [TrackAct::Dislike, TrackAct::Ban]);
}

/// What cannot work for a track is not in its menu at all: no album,
/// no artist, a song that cannot play, «Удалить навсегда» without a
/// file, «Скачать» of a track already on its way. A list's row always
/// offers «Удалить из библиотеки» (a stream track in a playlist is in
/// the library too).
#[test]
fn what_cannot_work_is_left_out() {
    let marks = nikgob_view::DislikesView::default();
    let bare = TrackRow { release_id: None, album: None, artists: Default::default(), ..track(false) };
    let acts = row_acts(&library_track_rows(&bare, of(&marks, false)));
    for a in [TrackAct::Album, TrackAct::Artist, TrackAct::AlbumDownload, TrackAct::Ban, TrackAct::Info] {
        assert!(!acts.contains(&a), "{a:?} in {acts:?}");
    }
    let mut rows = library_track_rows(&bare, of(&marks, false));
    push_deletes(&mut rows, &bare);
    let acts = row_acts(&rows);
    assert!(acts.contains(&TrackAct::Trash) && !acts.contains(&TrackAct::Purge), "no file: no delete for good");
    // «В библиотеку» of a song outside it, right after «В плейлист…»;
    // «Убрать из библиотеки» of one in it without a file or a like;
    // neither for a file or a liked song.
    let at = |acts: &[TrackAct], a| acts.iter().position(|x| *x == a);
    assert!(acts.contains(&TrackAct::OutOfLibrary) && !acts.contains(&TrackAct::ToLibrary));
    let heard = row_acts(&library_track_rows(&TrackRow { in_library: false, ..bare.clone() }, of(&marks, false)));
    assert_eq!(at(&heard, TrackAct::ToLibrary), at(&heard, TrackAct::Playlist).map(|p| p + 1));
    assert!(!heard.contains(&TrackAct::OutOfLibrary));
    for t in [track(true), TrackRow { liked: true, ..track(false) }] {
        let acts = row_acts(&library_track_rows(&t, of(&marks, false)));
        assert!(!acts.contains(&TrackAct::ToLibrary) && !acts.contains(&TrackAct::OutOfLibrary));
    }
    let unplayable = row_acts(&track_rows(TrackMenuOf { playable: false, ..of_stream() }));
    assert!(!unplayable.contains(&TrackAct::Next) && !unplayable.contains(&TrackAct::Queue));
    // On its way: no «Скачать»; waiting or not found: «Скачать».
    let t = track(false);
    let wished = |st| row_acts(&library_track_rows(&t, LibraryRowsOf { wished: Some(st), ..of(&marks, false) }));
    assert!(!wished(nikgob_view::WishlistStatus::Downloading).contains(&TrackAct::Fetch));
    assert!(wished(nikgob_view::WishlistStatus::Waiting).contains(&TrackAct::Fetch));
}

fn of_stream() -> TrackMenuOf {
    TrackMenuOf {
        playable: true,
        has_album: false,
        has_artists: false,
        has_file: false,
        also_on: None,
        info: false,
        fetch: None,
        better: false,
        pick_release: false,
        find_song: None,
        wrong_file: false,
        dislike: MenuItem::new("Не нравится"),
        ban: None,
        library: None,
    }
}

/// A track with a file: «Найти качество лучше»; without one and with a
/// torrent source: «Выбрать раздачу…» (the album page's name), which a
/// player with no torrent source does not offer.
#[test]
fn the_better_copy_and_the_release_by_hand_are_two_items() {
    let marks = nikgob_view::DislikesView::default();
    let file = |t: &TrackRow, torrents: bool| {
        let rows = library_track_rows(t, LibraryRowsOf { torrents, ..of(&marks, false) });
        row_acts(sub_rows(&rows, SubId::File).unwrap())
    };
    assert!(file(&track(true), true).contains(&TrackAct::Better));
    assert!(!file(&track(true), true).contains(&TrackAct::PickRelease));
    assert_eq!(file(&track(false), true), [TrackAct::AlbumDownload, TrackAct::PickRelease]);
    assert_eq!(file(&track(false), false), [TrackAct::AlbumDownload]);
    let rows = library_track_rows(&track(false), LibraryRowsOf { torrents: true, ..of(&marks, false) });
    assert!(row_items(&rows).iter().any(|(a, i)| *a == TrackAct::PickRelease && i.label == i18n::menus_in(i18n::Locale::Ru).pick_release));
    assert_eq!(TrackAct::PickRelease.menu_item(false), Some(TrackMenuItem::PickRelease));
    assert_eq!(TrackAct::Better.menu_item(true), Some(TrackMenuItem::Torrents));
}

/// «Разные песни» cannot be undone: it is last, after its divider.
#[test]
fn the_songs_apart_is_last() {
    let marks = nikgob_view::DislikesView::default();
    let also = (ReleaseId::new(), "Live".to_owned());
    let rows = library_track_rows(&track(true), LibraryRowsOf { also: Some(&also), ..of(&marks, false) });
    assert!(matches!(rows.last(), Some(Row::Item(TrackAct::Apart, _))));
    assert!(matches!(rows[rows.len() - 2], Row::Divider));
    let acts = row_acts(&rows);
    let album = acts.iter().position(|a| *a == TrackAct::Album).unwrap();
    assert_eq!(&acts[album..album + 3], [TrackAct::Album, TrackAct::AlsoOn, TrackAct::Artist]);
}

/// «Превратить в MP3» is in «Файл», right after «Найти качество
/// лучше», only for a file whose row says it surely holds an MP3.
#[test]
fn only_a_sure_fake_offers_its_mp3() {
    let row = |quality: Option<&str>, has_local: bool| TrackRow { quality: quality.map(str::to_owned), ..track(has_local) };
    let marks = nikgob_view::DislikesView::default();
    let file = |t: &TrackRow| row_acts(sub_rows(&library_track_rows(t, of(&marks, true)), SubId::File).unwrap());
    let fake = file(&row(Some("FLAC из MP3 ~128"), true));
    let at = fake.iter().position(|a| *a == TrackAct::Better).unwrap();
    assert_eq!(fake[at + 1], TrackAct::ToMp3);
    for (q, local) in [(Some("FLAC"), true), (None, true), (Some("MP3 320 из поддельного FLAC"), true), (Some("FLAC из MP3 ~128"), false)] {
        assert!(!file(&row(q, local)).contains(&TrackAct::ToMp3), "{q:?} {local}");
    }
}

/// «Обновить обложку» of a file's album is the last of the file's own
/// items in «Файл»; a song without an album or without a file has none.
#[test]
fn the_cover_of_a_files_album_is_refreshed_from_its_file_menu() {
    let marks = nikgob_view::DislikesView::default();
    let file = |t: &TrackRow| row_acts(sub_rows(&library_track_rows(t, of(&marks, true)), SubId::File).unwrap());
    assert_eq!(file(&track(true)).last(), Some(&TrackAct::RefreshCover));
    let loose = TrackRow { release_id: None, album: None, ..track(true) };
    assert!(!file(&loose).contains(&TrackAct::RefreshCover));
    assert!(!file(&track(false)).contains(&TrackAct::RefreshCover));
    assert_eq!(TrackAct::RefreshCover.menu_item(true), None);
    let full = track(true);
    let rows = library_track_rows(&full, of(&marks, true));
    let items = row_items(&rows);
    assert!(items.iter().any(|(a, i)| *a == TrackAct::RefreshCover && i.label == i18n::menus_in(i18n::Locale::Ru).refresh_cover));
}

/// «Меню трека»: an item sent to «Файл» joins the submenu in the menu's
/// order, one taken out stands right before it, a hidden one is gone;
/// nothing set, nothing changes.
#[test]
fn the_settings_place_the_items_of_the_menu() {
    use nikgob_view::{TrackMenuItem as I, TrackMenuPlace as P, TrackMenuSlot};
    use TrackAct as A;
    let t = track(true);
    let marks = nikgob_view::DislikesView::default();
    let library = || {
        let mut rows = with_speed_row(library_track_rows(&t, of(&marks, true)), &speeds(true, &[]), true);
        push_deletes(&mut rows, &t);
        rows
    };
    let with = |slots: &[(I, P)]| {
        let slots: Vec<TrackMenuSlot> = slots.iter().map(|&(item, place)| TrackMenuSlot { item, place }).collect();
        placed(library(), &slots, |a| a.menu_item(true))
    };
    let top = |rows: &[Row<A>]| -> Vec<A> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Item(a, _) => Some(*a),
                _ => None,
            })
            .collect()
    };
    let file = |rows: &[Row<A>]| row_acts(sub_rows(rows, SubId::File).unwrap());
    assert_eq!(row_acts(&with(&[])), row_acts(&library()));
    let plain = (top(&library()), file(&library()));
    assert_eq!(plain.1[..3], [A::Info, A::Fetch, A::Better]);

    // Into «Файл»: before its own items, in the menu's order; from
    // below it: after them, before the deletes.
    let rows = with(&[(I::Share, P::File), (I::Radio, P::File), (I::Ban, P::File)]);
    assert!(!top(&rows).contains(&A::Share) && !top(&rows).contains(&A::Radio) && !top(&rows).contains(&A::Ban));
    let f = file(&rows);
    assert_eq!(f[..4], [A::Radio, A::Share, A::Info, A::Fetch]);
    assert_eq!(f[f.len() - 3..], [A::Ban, A::Trash, A::Purge]);
    assert!(matches!(sub_rows(&rows, SubId::File).unwrap()[f.len() - 2], Row::Divider));

    // Hidden: not built at all.
    let rows = with(&[(I::Info, P::Hidden), (I::Torrents, P::Hidden)]);
    let acts = row_acts(&rows);
    assert!(!acts.contains(&A::Info) && !acts.contains(&A::Better));
    assert_eq!(acts.len(), row_acts(&library()).len() - 2);

    // «Редактировать теги…» taken out: in the menu, right before «Файл».
    let rows = with(&[(I::Tags, P::Menu)]);
    let at = rows.iter().position(|r| matches!(r, Row::Sub(..))).unwrap();
    assert!(matches!(rows[at - 1], Row::Item(A::Fetch, _)) && matches!(rows[at - 2], Row::Item(A::Speed, _)));
    assert_eq!(file(&rows)[..2], [A::Info, A::Better]);

    // Everything it places taken out of «Файл»: the album's cover and
    // the deletes stay in it.
    let rows = with(&[(I::Info, P::Menu), (I::Tags, P::Menu), (I::Torrents, P::Menu), (I::WrongFile, P::Hidden)]);
    assert_eq!(file(&rows), [A::RefreshCover, A::Trash, A::Purge]);
    // No divider is doubled or left at an end once the menu is built.
    let (_, acts) = widgets::menu_of("placed-test", rows);
    assert!(acts.contains(&A::Fetch) && acts.contains(&A::Trash));
}

/// «Источник и качество» of a list's row: of any track with a file or
/// a source to stream from, whichever plays.
#[test]
fn the_info_is_of_any_track_that_has_one() {
    let state = ViewState::default();
    let wish = widgets::WishIndex::default();
    let marks = nikgob_view::DislikesView::default();
    let file = track(true);
    let stream = TrackRow { source: Some(nikgob_view::SourceKind::Spotify), ..track(false) };
    let nothing = track(false);
    for (t, info) in [(&file, true), (&stream, true), (&nothing, false)] {
        assert_eq!(LibraryRowsOf::of(&state, t, &wish).info, info, "{:?}", t.source);
        let acts = row_acts(&library_track_rows(t, LibraryRowsOf { info: has_info(t), ..of(&marks, false) }));
        assert_eq!(acts.contains(&TrackAct::Info), info);
    }
    assert!(!is_playing(&state, file.id));
}

/// On a page, a track's menu leaves out what leads back to it: no «К
/// альбому» on the page of its album, no «К исполнителю» on its
/// artist's (one with another artist goes to that one), wherever
/// «Меню трека» placed them; anywhere else both stay.
#[test]
fn a_menu_leaves_out_the_page_it_is_on() {
    use nikgob_view::{ArtistId, Credit, CreditName, StreamPage};
    let marks = nikgob_view::DislikesView::default();
    let t = track(true);
    let album = t.release_id.unwrap();
    let artist = *t.artists.main().unwrap();
    let acts = |here: &Here, t: &TrackRow| row_acts(&here.trim(library_track_rows(t, of(&marks, false)), t, None, Some));

    let anywhere = acts(&Here::default(), &t);
    assert!(anywhere.contains(&TrackAct::Album) && anywhere.contains(&TrackAct::Artist));
    for route in [Route::Home, Route::Library(LibraryTab::Tracks), Route::Playlist(nikgob_view::PlaylistId::new())] {
        assert_eq!(acts(&Here::of(&route), &t), anywhere, "{route:?}");
    }
    // Another album's or artist's page: both stay.
    assert_eq!(acts(&Here::of(&Route::Release(ReleaseId::new())), &t), anywhere);
    assert_eq!(acts(&Here::of(&Route::Artist(ArtistId::new())), &t), anywhere);

    let on_album = acts(&Here::of(&Route::Release(album)), &t);
    assert!(!on_album.contains(&TrackAct::Album) && on_album.contains(&TrackAct::Artist));
    // The album's download is no way to its page: it stays.
    let missing = track(false);
    let on_its_album = Here::of(&Route::Release(missing.release_id.unwrap()));
    let file = |rows: Vec<Row<TrackAct>>| row_acts(sub_rows(&rows, SubId::File).unwrap());
    assert!(file(on_its_album.trim(library_track_rows(&missing, of(&marks, false)), &missing, None, Some)).contains(&TrackAct::AlbumDownload));

    let on_artist = acts(&Here::of(&Route::Artist(artist)), &t);
    assert!(on_artist.contains(&TrackAct::Album) && !on_artist.contains(&TrackAct::Artist));
    // With a guest: «К исполнителю» goes to the guest.
    let guest = ArtistId::new();
    let duet = TrackRow {
        artists: Credit(vec![
            CreditName { name: "Artist".into(), id: Some(artist), join: " feat. ".into() },
            CreditName { name: "Guest".into(), id: Some(guest), join: String::new() },
        ]),
        ..track(true)
    };
    let here = Here::of(&Route::Artist(artist));
    assert!(acts(&here, &duet).contains(&TrackAct::Artist));
    assert_eq!(here.artists(&duet.artists), vec![("Guest".to_owned(), Route::Artist(guest))]);
    assert_eq!(Here::default().artists(&duet.artists).len(), 2);

    // Moved into «Файл» by «Меню трека»: left out there too.
    use nikgob_view::{TrackMenuItem as I, TrackMenuPlace as P, TrackMenuSlot};
    let slots = [TrackMenuSlot { item: I::Album, place: P::File }];
    let rows = placed(library_track_rows(&t, of(&marks, false)), &slots, |a| a.menu_item(true));
    assert!(row_acts(sub_rows(&rows, SubId::File).unwrap()).contains(&TrackAct::Album));
    let rows = Here::of(&Route::Release(album)).trim(rows, &t, None, Some);
    assert!(!row_acts(&rows).contains(&TrackAct::Album));

    // «Есть и на альбоме» of the album on screen: left out, «Разные
    // песни» stays.
    let other = ReleaseId::new();
    let also = (other, "Live".to_owned());
    let rows = library_track_rows(&t, LibraryRowsOf { also: Some(&also), ..of(&marks, false) });
    let rows = row_acts(&Here::of(&Route::Release(other)).trim(rows, &t, Some(other), Some));
    assert!(!rows.contains(&TrackAct::AlsoOn) && rows.contains(&TrackAct::Apart) && rows.contains(&TrackAct::Album));

    // A service's page: its album and its artist.
    let remote = |id: &str| nikgob_view::RemoteId::new(nikgob_view::Service::Spotify, id);
    let song = nikgob_view::StreamTrackRow {
        remote: remote("t"),
        title: "Song".into(),
        artist: "Band".into(),
        artists: Credit::single("Band", Some(remote("a"))),
        album: Some("LP".into()),
        album_id: Some(remote("l")),
        duration: None,
        disc_no: 1,
        track_no: 1,
        cover_url: None,
        kind: nikgob_view::TrackKind::Song,
        playable: true,
        track: None,
        liked: false,
        in_library: false,
        has_local: false,
        temporary: false,
        elsewhere: None,
        also_on: Vec::new(),
    };
    let keeps = |route: Route, act| Here::of(&route).keeps_stream(act, &song);
    assert!(!keeps(Route::Stream(StreamPage::Album(remote("l"))), TrackAct::Album));
    assert!(keeps(Route::Stream(StreamPage::Album(remote("l"))), TrackAct::Artist));
    assert!(!keeps(Route::Stream(StreamPage::Artist(remote("a"))), TrackAct::Artist));
    assert!(keeps(Route::Stream(StreamPage::Artist(remote("a"))), TrackAct::Album));
    assert!(keeps(Route::Stream(StreamPage::Album(remote("other"))), TrackAct::Album));
}

/// The page drawn this frame or the one before is where the menus are;
/// once no page is drawn (Now Playing covers it), nowhere.
#[test]
fn the_page_on_screen_is_kept_for_a_frame() {
    let ctx = Context::default();
    let album = ReleaseId::new();
    let frame = |set: bool| {
        let mut seen = Here::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if set {
                set_here(ui.ctx(), Here::of(&Route::Release(album)));
            }
            seen = here(ui.ctx());
        });
        out.textures_delta.clear();
        seen
    };
    assert_eq!(frame(true).album, Some(album));
    assert_eq!(frame(false).album, Some(album), "the frame after it");
    frame(false);
    assert_eq!(frame(false), Here::default(), "no page for two frames: none");
}
