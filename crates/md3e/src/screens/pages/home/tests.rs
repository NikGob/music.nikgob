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
    assert!(!sections(&h, true).contains(&Section::New), "no section left only to say why it is empty");
    assert!(!sections(&h, false).contains(&Section::New));
}

#[test]
fn an_empty_home_is_one_line_and_its_buttons() {
    let page = texts(&ViewState::default(), 1280.0);
    assert!(page.contains(EMPTY_LINE));
    assert!(page.contains("Найти музыку") && page.contains("Добавить папку…"));
    assert!(!page.contains("Найди песню") && !page.contains("Здесь соберётся"));
}

/// A discovery's menu is a service song's one list (the same rows in the
/// same order), then its own «В любимое».
#[test]
fn a_discovery_has_the_service_song_menu_and_its_like() {
    use crate::screens::pages::{TrackAct as A, TrackMenuOf, track_rows, widgets::row_acts};
    use crate::components::MenuItem;
    let song = || {
        track_rows(TrackMenuOf {
            playable: true,
            has_album: true,
            has_artists: true,
            also_on: None,
            info: false,
            fetch: MenuItem::new("Скачать"),
            torrents: None,
            find_song: None,
            wrong_file: false,
            dislike: MenuItem::new("Не нравится"),
            ban: MenuItem::new("Не рекомендовать исполнителя"),
        })
    };
    let list: Vec<DiscoverAct> = row_acts(&song()).into_iter().map(DiscoverAct::Track).collect();
    let menu = row_acts(&with_like(song(), false));
    assert_eq!(&menu[..list.len()], &list[..]);
    assert_eq!(menu.last(), Some(&DiscoverAct::Like));
    assert_eq!(&menu[..4], [A::Next, A::Queue, A::Playlist, A::Radio].map(DiscoverAct::Track));
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
    assert_eq!(Section::ALL.len(), 8);
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
fn day_lane(n: u8) -> LaneCard {
    LaneCard { key: nikgob_view::MixKey::Daily(n), ..lane(&format!("Микс дня {n}")) }
}

/// «Миксы дня» are a row of their own beside «Миксы для тебя»: each row
/// takes its mixes by their keys, and both are drawn under their names at
/// any width (what does not fit goes on sideways).
#[test]
fn the_mixes_of_the_day_have_their_own_row() {
    let mut h = HomeView { lanes: vec![lane("Всё вперемешку")], ..HomeView::default() };
    assert_eq!(sections(&h, false), [Section::Mixes]);
    h.lanes = (1..=6).map(day_lane).collect();
    assert_eq!(sections(&h, false), [Section::Daily]);
    h.lanes.push(lane("Любимые"));
    assert_eq!(sections(&h, false), [Section::Daily, Section::Mixes]);
    let mut st = state(Vec::new(), false);
    st.home.lanes = h.lanes;
    for width in [900.0, 1280.0, 1700.0] {
        let page = texts(&st, width);
        assert!(page.contains("Миксы дня") && page.contains("Миксы для тебя"), "{width}");
        assert!(page.contains("Микс дня 1") && page.contains("Любимые"), "{width}");
    }
    // Turned off in «Разделы главной», the row is gone and the other stays.
    nikgob_view::PrefPatch::HomeSections(vec![nikgob_view::HomeSectionSwitch { key: nikgob_view::HomeSection::Daily, on: false }]).apply(&mut st.prefs);
    let page = texts(&st, 1280.0);
    assert!(!page.contains("Миксы дня") && page.contains("Миксы для тебя"));
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
