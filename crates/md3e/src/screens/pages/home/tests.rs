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
        // And on, the card is there and the page under it moves down.
        let (on, on_card) = picture(&state(vec![world("mia"), world("omori")], true), width);
        assert!(on_card);
        assert_ne!(base, on);
    }
}

#[test]
fn one_world_is_the_card_without_its_row() {
    let (one, shown) = picture(&state(vec![world("mia")], true), 1440.0);
    assert!(shown);
    let (two, _) = picture(&state(vec![world("mia"), world("omori")], true), 1440.0);
    assert!(two.len() > one.len(), "two worlds draw their row of tiles");
}
