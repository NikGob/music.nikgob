use super::*;
use egui::{Context, RawInput};
use nikgob_view::{ReleaseId, TrackId, TrackRow, World, WorldCard, WorldGo, WorldIcon, WorldKind, WorldStart};

fn context() -> Context {
    let ctx = Context::default();
    let mut fonts = egui::FontDefinitions::default();
    crate::typography::install_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    crate::icons::install(&ctx);
    ctx
}

fn song(title: &str) -> TrackRow {
    TrackRow {
        id: TrackId::new(),
        title: title.into(),
        artist: "Kevin Penkin".into(),
        album: Some("Made in Abyss OST".into()),
        release_id: None,
        artists: Default::default(),
        disc_no: 1,
        track_no: 1,
        duration: None,
        quality: None,
        source: None,
        has_local: true,
        liked: false,
        cover: None,
        clip: false,
        in_library: true,
    }
}

fn world(key: &str) -> World {
    let tracks: Vec<TrackId> = (0..3).map(|_| TrackId::new()).collect();
    World {
        key: key.into(),
        title: key.to_uppercase(),
        release: Some(ReleaseId::new()),
        kind: WorldKind::Soundtrack,
        cover: None,
        cover_hd: None,
        kicker: "Слушал вчера".into(),
        facts: "Саундтрек · Kevin Penkin · 4 альбома · 134 трека".into(),
        heard: 36,
        total: 134,
        start: WorldStart::Finish,
        playing: false,
        primary: tracks.clone(),
        primary_tip: "98 треков, 4 ч 35 мин: по порядку альбомов".into(),
        shuffled: tracks.clone(),
        albums: Vec::new(),
        cards: vec![WorldCard { icon: WorldIcon::Play, kicker: "Продолжить".into(), title: "OST 4, трек 20".into(), go: WorldGo::Play(tracks) }],
    }
}

/// A home with a shelf of songs (and `worlds` for the card).
fn state(worlds: Vec<World>, card: bool) -> ViewState {
    let mut state = ViewState::default();
    state.home.listen_again = (0..6).map(|i| song(&format!("Song {i}"))).collect();
    state.home.history = true;
    state.home.worlds = worlds;
    state.prefs.home_card = card;
    state
}

/// The page's picture after a few frames (the entrance played out).
fn picture(state: &ViewState, width: f32) -> (Vec<String>, bool) {
    let ctx = context();
    let mut pages = PageState::default();
    let mut shapes = Vec::new();
    for i in 0..40 {
        let mut out = Vec::new();
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 900.0))),
                time: Some(f64::from(i) * 0.1),
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    let _ = show(ui, state, &mut pages, &mut out);
                });
            },
        );
        output.textures_delta.clear();
        shapes = output.shapes.iter().map(|s| format!("{:?}", s.shape)).collect();
    }
    (shapes, worlds::shown(&ctx))
}

#[test]
fn shortcuts_follow_the_width() {
    assert_eq!(shortcuts_shown(3), 6);
    assert_eq!(shortcuts_shown(4), 8);
}

#[test]
fn without_the_card_the_page_is_the_page_without_worlds() {
    for width in [1280.0, 1440.0] {
        let (base, base_card) = picture(&state(Vec::new(), true), width);
        let (off, off_card) = picture(&state(vec![world("mia"), world("omori")], false), width);
        assert!(!base_card && !off_card);
        assert_eq!(base, off, "the setting off must leave the saved base page at {width}");
        // And on, the card is there and the page under it moves down; with
        // the card out of the app, the setting on changes nothing.
        let (on, on_card) = picture(&state(vec![world("mia"), world("omori")], true), width);
        if nikgob_view::HOME_CARD {
            assert!(on_card);
            assert_ne!(base, on);
        } else {
            assert!(!on_card);
            assert_eq!(base, on, "the card is out of the app at {width}");
        }
    }
}

#[test]
fn one_world_is_the_card_without_its_row() {
    if !nikgob_view::HOME_CARD {
        return;
    }
    let (one, shown) = picture(&state(vec![world("mia")], true), 1440.0);
    assert!(shown);
    let (two, _) = picture(&state(vec![world("mia"), world("omori")], true), 1440.0);
    assert!(two.len() > one.len(), "two worlds draw their row of tiles");
}

#[test]
fn the_greeting_names_the_user_when_it_knows_the_name() {
    assert_eq!(greeting_line(19, ""), "Добрый вечер");
    assert_eq!(greeting_line(19, "   "), "Добрый вечер", "no name, no comma");
    assert_eq!(greeting_line(19, "  Ник  "), "Добрый вечер, Ник");
    assert_eq!(greeting_line(8, "Ник"), "Доброе утро, Ник");
    assert_eq!(greeting_line(13, "Ник"), "Добрый день, Ник");
    assert_eq!(greeting_line(2, "Ник"), "Доброй ночи, Ник");
    let long = "Николай Константинович Рерих-Младший Третий";
    let line = greeting_line(19, long);
    assert_eq!(line.chars().count(), "Добрый вечер, ".chars().count() + nikgob_view::USER_NAME_MAX);
}

/// The text of every galley the page paints.
fn texts(state: &ViewState, width: f32) -> String {
    picture(state, width).0.join("\n")
}

#[test]
fn the_name_is_asked_once_on_the_first_start() {
    let mut first = state(Vec::new(), false);
    first.prefs.name_prompt = true;
    assert!(texts(&first, 1200.0).contains("Как к тебе обращаться?"));
    // «Пропустить» or a name: asked no more.
    let mut skipped = first.clone();
    nikgob_view::PrefPatch::NamePrompt(false).apply(&mut skipped.prefs);
    assert!(!texts(&skipped, 1200.0).contains("Как к тебе обращаться?"));
    let mut named = first.clone();
    nikgob_view::PrefPatch::UserName("Ник".into()).apply(&mut named.prefs);
    let page = texts(&named, 1200.0);
    assert!(!page.contains("Как к тебе обращаться?"));
    assert!(page.contains(", Ник"), "the greeting names the user");
    // Not asked of a library from before (its prefs file had no question).
    assert!(!texts(&state(Vec::new(), false), 1200.0).contains("Как к тебе обращаться?"));
}

/// A mix of «Миксы для тебя» without covers.
fn lane(title: &str) -> LaneCard {
    LaneCard {
        key: nikgob_view::MixKey::Everything,
        title: title.to_owned(),
        short_titles: Vec::new(),
        artists: vec!["Kevin Penkin".to_owned()],
        new_artists: Vec::new(),
        covers: Vec::new(),
        new_cover: None,
        tracks: Vec::new(),
        new_tracks: Vec::new(),
        blend_base: None,
        queue: None,
        why: None,
    }
}

#[test]
fn the_home_has_no_explanatory_lines() {
    let quiet = [
        "Разбираю звук",
        "Собраны из того",
        "в миксах только треки",
        "Новое скрыто",
        "Показать новое",
        "Имя будет",
        "Примерно каждый",
        "Без нового",
    ];
    let mut st = state(Vec::new(), false);
    st.home.lanes = vec![lane("Всё вперемешку")];
    st.home.history = false;
    st.analysis = nikgob_view::AnalysisProgress { files_done: 1, files_total: 10, streams_done: 0, streams_total: 0 };
    st.prefs.name_prompt = true;
    for local_only in [false, true] {
        st.prefs.mixes_downloaded_only = local_only;
        let page = texts(&st, 1280.0);
        assert!(page.contains("Миксы для тебя") && page.contains("Как к тебе обращаться?"), "titles stay ({local_only})");
        for q in quiet {
            assert!(!page.contains(q), "«{q}» on the home with «Только скачанное» {local_only}");
        }
    }
}

#[test]
fn downloaded_only_takes_the_new_music_away_without_a_word() {
    let h = HomeView::default();
    assert!(!sections(&h, false, true).contains(&Section::New), "no section left only to say why it is empty");
    assert!(!sections(&h, false, false).contains(&Section::New));
}

#[test]
fn an_empty_home_is_one_line_and_the_ways_in() {
    let mut st = ViewState::default();
    st.home.library_empty = true;
    for width in [1280.0, 720.0] {
        let page = texts(&st, width);
        assert!(page.contains(EMPTY_LINE), "{width}");
        for way in ["Spotify", "YouTube Music", "SoundCloud", "Добавить папку", "Найти музыку"] {
            assert!(page.contains(way), "«{way}» at {width}");
        }
        assert!(!page.contains("Найди песню") && !page.contains("Здесь соберётся"));
    }
}

/// A library with music whose rows came out empty (a few songs, partial
/// albums) never says there is no music: the greeting, and the ways in
/// under it without the line.
#[test]
fn a_library_with_music_but_no_rows_is_not_called_empty() {
    let st = ViewState::default();
    assert!(!st.home.library_empty);
    let page = texts(&st, 1280.0);
    assert!(!page.contains(EMPTY_LINE));
    assert!(page.contains("Добр") && page.contains("Добавить папку"));
}

/// «Только скачанное» with nothing downloaded keeps its header (and the
/// chip that turns it off) and shows the ways in under it, without the
/// line, whether or not the library has anything at all.
#[test]
fn downloaded_only_with_nothing_keeps_the_chip_over_the_ways_in() {
    for empty in [false, true] {
        let mut st = ViewState::default();
        st.prefs.mixes_downloaded_only = true;
        st.home.library_empty = empty;
        let page = texts(&st, 1280.0);
        assert!(page.contains("Только скачанное") && page.contains("Добавить папку"), "{empty}");
        assert!(!page.contains(EMPTY_LINE), "{empty}");
    }
}

/// A discovery's menu is a service song's one list, in its order, with no
/// like of its own: the row's ♥ is the like.
#[test]
fn a_discovery_has_the_service_song_menu_without_a_like() {
    use crate::screens::pages::{TrackAct as A, TrackMenuOf, track_rows, widgets::row_acts};
    use crate::components::MenuItem;
    let menu = row_acts(&track_rows(TrackMenuOf {
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
    }));
    assert_eq!(&menu[..4], [A::Next, A::Queue, A::Radio, A::Playlist]);
    assert_eq!(&menu[menu.len() - 2..], [A::Dislike, A::Ban]);
}

/// «Разделы главной»: its order first, the sections it turns off gone, a
/// section it does not name after the rest; nothing set, nothing changes.
#[test]
fn sections_follow_their_order_and_switches() {
    use Section as S;
    let shown = vec![S::Shortcuts, S::Mixes, S::ListenAgain, S::New];
    assert_eq!(arrange(shown.clone(), &[]), shown);
    let order = [(S::New, true), (S::Shortcuts, true), (S::Mixes, false), (S::Releases, true)];
    assert_eq!(arrange(shown.clone(), &order), vec![S::New, S::Shortcuts, S::ListenAgain]);
    // A section named twice is drawn once.
    assert_eq!(arrange(vec![S::Mixes], &[(S::Mixes, true), (S::Mixes, true)]), vec![S::Mixes]);
    assert_eq!(Section::ALL.len(), 7);
    // A list saved with the row of the day's mixes: they are in «Миксы для
    // тебя» now.
    assert_eq!(section_of(nikgob_view::HomeSection::Daily), S::Mixes);
    assert_eq!(Section::ALL.len(), nikgob_view::HomeSection::ALL.len());
    for (s, key) in Section::ALL.iter().zip(nikgob_view::HomeSection::ALL) {
        assert_eq!(section_of(key), *s);
        assert_eq!(s.title(), key.label());
    }
    assert!(Section::ALL.iter().all(|s| !s.title().is_empty()));
    // Until «Разделы главной» are wired, the page keeps its own order.
    assert!(order_of(&nikgob_view::Prefs::default()).is_empty());
}

/// A mix of the day.
fn day_lane(n: u8, title: &str) -> LaneCard {
    LaneCard { key: nikgob_view::MixKey::Daily(n), ..lane(title) }
}

/// The day's mixes stand in «Миксы для тебя» with the others, in one row
/// at any width (what does not fit goes on sideways): there is no row of
/// numbered mixes. «Любимые», the playlist, is first while anything is
/// liked, and makes the row by itself.
#[test]
fn the_mixes_are_one_row() {
    let mut h = HomeView { lanes: vec![lane("Всё вперемешку")], ..HomeView::default() };
    assert_eq!(sections(&h, false, false), [Section::Mixes]);
    h.lanes = vec![day_lane(0, "Микс дня"), LaneCard { key: nikgob_view::MixKey::Genre(7), ..lane("Хип-хоп микс") }, lane("На вечер")];
    assert_eq!(sections(&h, false, false), [Section::Mixes]);
    assert!(sections(&HomeView::default(), false, false).is_empty());
    assert_eq!(sections(&HomeView::default(), true, false), [Section::Mixes]);
    let mut st = state(Vec::new(), false);
    st.home.lanes = h.lanes;
    assert!(liked_playlist(&st).is_none());
    for width in [900.0, 1280.0, 1700.0] {
        let page = texts(&st, width);
        assert!(!page.contains("Миксы дня") && page.contains("Миксы для тебя"), "{width}");
        assert!(page.contains("Микс дня") && page.contains("Хип-хоп микс"), "{width}");
        assert!(!page.contains("Любимые"), "{width}");
    }
    // Nothing liked: no tile. Something liked: the playlist, first.
    let liked = |n: u32| nikgob_view::PlaylistCard {
        id: nikgob_view::LIKED_PLAYLIST,
        name: "Любимые".into(),
        cover: None,
        track_count: n,
        pinned: false,
        cover_mode: Default::default(),
        service: None,
    };
    st.library.playlists = vec![liked(0)];
    assert!(liked_playlist(&st).is_none());
    assert!(!texts(&st, 1280.0).contains("Любимые"));
    st.library.playlists = vec![liked(5)];
    assert_eq!(liked_playlist(&st).map(|p| p.track_count), Some(5));
    let page = texts(&st, 1280.0);
    assert!(page.contains("Любимые") && page.contains("5 треков") && page.contains("На вечер"));
    // «Моя волна» right after it, before the mixes (no second one in the header).
    let (liked_at, wave_at, mix_at) = (page.find("5 треков").unwrap(), page.rfind("Моя волна").unwrap(), page.find("На вечер").unwrap());
    assert!(liked_at < wave_at && wave_at < mix_at, "{page}");
    assert_eq!(page.matches("Моя волна").count(), 1, "only the card");
}

/// «Слушать снова» and «Новое для тебя» stand in columns of two songs,
/// which go on sideways like the cards.
#[test]
fn the_songs_stand_in_columns() {
    assert_eq!(song_columns(0), (0, 0));
    assert_eq!(song_columns(1), (1, 1));
    assert_eq!(song_columns(2), (1, 2));
    assert_eq!(song_columns(12), (6, 2));
    assert_eq!(song_columns(13), (7, 2));
    let cell = Rect::from_min_size(pos2(10.0, 100.0), vec2(300.0, 2.0 * ROW_H));
    assert_eq!(song_cell(cell, 1), Rect::from_min_size(pos2(10.0, 100.0 + ROW_H), vec2(300.0, ROW_H)));
    // Twelve songs at a width of three columns: the first six are on the
    // page, the rest wait at the right.
    let mut st = state(Vec::new(), false);
    st.home.listen_again = (0..12).map(|i| song(&format!("Song {i}"))).collect();
    let page = texts(&st, 1280.0);
    assert!(page.contains("Song 0") && page.contains("Song 5"));
    assert!(!page.contains("Song 11"), "the last column is out of the row's view");
}

/// Tiles of the day that fit share the row; more keep the row's count and
/// go on sideways.
#[test]
fn the_tiles_of_the_day_fill_their_row() {
    assert_eq!(of_day_per(4, 3), 3);
    assert_eq!(of_day_per(4, 4), 4);
    assert_eq!(of_day_per(4, 5), 4);
    assert_eq!(of_day_per(2, 5), 2);
    assert_eq!(of_day_per(2, 1), 1);
    assert_eq!(of_day_per(4, 0), 1);
}

/// The page of a window `w` wide with the collapsed rail (96) and the
/// window's margins (2 × 24) off.
fn page_of(window: f32) -> Grid {
    Grid::new(0.0, window - 96.0 - 48.0)
}

/// A narrow page keeps three cards a row as before; from the expanded
/// class on a card is at most `CARD_MAX` wide and a wider window shows
/// more of them.
#[test]
fn cards_keep_their_size_on_a_wide_window() {
    for width in [360.0, 572.0, 695.0] {
        let g = Grid::new(0.0, width);
        assert_eq!(g.cards_per_line(), 3, "{width}");
        assert_eq!(g.rows_per_line(), 2, "{width}");
        assert_eq!(g.of_day_per_line(), 2, "{width}");
        assert_eq!(g.of_day_least(), 1, "{width}");
    }
    // (window, mix and album cards in a row, their width)
    for (window, n, w) in [(1280.0, 6, 176.0), (1920.0, 9, 183.11), (2560.0, 12, 186.67), (3840.0, 12, 198.67)] {
        let g = page_of(window);
        assert_eq!(g.cards_per_line(), n, "{window}");
        let tile = g.tile(n);
        assert!((tile - w).abs() < 0.01, "{window}: {tile}");
        assert!(tile <= CARD_MAX + 0.01, "{window}");
        // The row fills the page: the last card ends on its right edge.
        let last = g.cell(n - 1, n, 0.0, 10.0, 0.0);
        assert!((last.right() - (g.left + g.width)).abs() < 0.01, "{window}");
        assert!((g.cell(1, n, 0.0, 10.0, 0.0).left() - (g.left + tile + GUTTER)).abs() < 0.01);
    }
    // Every page from the expanded class up: no card past the most.
    let mut width = CARDS_CAPPED_FROM;
    while width < 3000.0 {
        let g = Grid::new(0.0, width);
        assert!(g.tile(g.cards_per_line()) <= CARD_MAX + 0.01, "{width}");
        width += 7.0;
    }
    // A wider window never has fewer cards.
    let counts: Vec<usize> = (700..2600).step_by(10).map(|w| Grid::new(0.0, w as f32).cards_per_line()).collect();
    assert!(counts.windows(2).all(|c| c[0] <= c[1]), "{counts:?}");
}

/// Song columns, shortcuts and the tiles of the day: as before up to four
/// to a row, then more columns of songs and tiles; the shortcuts stay at
/// four (the eight the runtime makes, in two rows).
#[test]
fn list_rows_add_columns_on_a_wide_window() {
    // (window, song columns, shortcuts, tiles of the day, fewest tiles)
    for (window, songs, shortcuts, tiles, least) in [
        (1000.0, 2, 2, 2, 1),
        (1280.0, 3, 3, 4, 2),
        (1920.0, 4, 4, 4, 4),
        (2560.0, 6, 4, 6, 5),
    ] {
        let g = page_of(window);
        assert_eq!(g.rows_per_line(), songs, "{window}");
        assert_eq!(g.shortcuts_per_line(), shortcuts, "{window}");
        assert_eq!(g.of_day_per_line(), tiles, "{window}");
        assert_eq!(g.of_day_least(), least, "{window}");
        assert!(g.of_day_least() <= g.of_day_per_line(), "{window}");
        if g.width >= FOUR_FROM {
            assert!(g.tile(g.rows_per_line()) <= ROW_MAX + 0.01, "{window}");
            assert!(g.tile(g.of_day_per_line()) <= OF_DAY_MAX + 0.01, "{window}");
        }
    }
    assert_eq!(shortcuts_shown(page_of(2560.0).shortcuts_per_line()), 8);
    // Two tiles of the day on a wide page keep a sensible width.
    let g = page_of(2560.0);
    let per = of_day_per(g.of_day_per_line(), 2).max(g.of_day_least());
    assert!(g.tile(per) <= OF_DAY_STRETCH + 0.01);
    // A narrow page: one tile alone fills the row, as before.
    let g = page_of(900.0);
    assert_eq!(of_day_per(g.of_day_per_line(), 1).max(g.of_day_least()), 1);
}

/// The page runs as wide as the window's content on any usual screen and
/// is centred past [`MAX_W`].
#[test]
fn the_page_is_centred_past_its_widest() {
    let g = Grid::new(100.0, 1792.0);
    assert_eq!((g.left, g.width), (100.0, 1792.0));
    let g = Grid::new(100.0, MAX_W + 400.0);
    assert_eq!((g.left, g.width), (300.0, MAX_W));
}

/// A shortcut built again with what its cover carries besides the picture
/// (a colour seed found since, its ambient picture) is the same card: it
/// does not come in anew; another picture is another card.
#[test]
fn a_shortcut_is_new_only_when_its_picture_or_name_is() {
    use nikgob_view::{ArtistCard, ArtistId, Cover, Shortcut};
    let cover = |path: &str, seed: Option<u32>, ambient: Option<&str>| Cover { path: path.into(), seed, ambient: ambient.map(Into::into) };
    let id = ArtistId::new();
    let artist = |name: &str, image: Option<Cover>, latin: Option<&str>| {
        Shortcut::Artist(ArtistCard {
            id,
            name: name.into(),
            image,
            liked: false,
            track_count: 3,
            latin: latin.map(Into::into),
            character: false,
        })
    };
    let before = shortcut_key(&artist("Lida", Some(cover("C:/a/lida.jpg", None, None)), None));
    let again = shortcut_key(&artist("Lida", Some(cover("C:/a/lida.jpg", Some(0xff10_2030), Some("C:/a/ambient.png"))), Some("Lida")));
    assert_eq!(before, again);
    assert_ne!(before, shortcut_key(&artist("Lida", Some(cover("C:/a/other.jpg", None, None)), None)));
    assert_ne!(before, shortcut_key(&artist("Лида", Some(cover("C:/a/lida.jpg", None, None)), None)));
    assert_eq!(shortcut_pictures(&artist("Lida", Some(cover("C:/a/lida.jpg", None, None)), None)).len(), 1);
    assert!(shortcut_pictures(&artist("Lida", None, None)).is_empty());
}

/// A tile of the day plays while the queue started from it plays (the
/// queue names it); a mix card plays while its wave or its list does,
/// whatever its click does.
#[test]
fn a_playing_tile_and_mix_are_told() {
    let mut st = ViewState::default();
    let tile = MixCard { id: MixId::Daylist, title: "Утро".into(), kicker: String::new(), subtitle: String::new(), covers: Vec::new() };
    assert!(!tile_playing(&st, &tile));
    st.player.status = Status::Playing;
    st.player.context = Some("микса «Утро»".into());
    assert!(tile_playing(&st, &tile));
    st.player.context = Some("микса «Утро 2»".into());
    assert!(!tile_playing(&st, &tile));
    st.player.context = Some("микса «Утро»".into());
    st.player.status = Status::Paused;
    assert!(!tile_playing(&st, &tile), "paused: at rest");
    let l = lane("Всё вперемешку");
    st.player.status = Status::Playing;
    st.player.context = Some(l.context());
    assert!(lane_playing(&st, &l), "started from its page, it plays though its click starts a wave");
}
