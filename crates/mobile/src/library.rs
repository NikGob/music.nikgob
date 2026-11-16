//! Библиотека (Metrolist's library): the search bar at the top, the chips
//! of what to show in a row under it — «Плейлисты», «Альбомы»,
//! «Исполнители», «Треки» — and with none of them the playlists over the
//! latest albums. A chip tapped again, or ✕ before the chips, goes back to
//! all of it. Under a chip, a row of its tools: the sort (a sheet) or a new
//! playlist on the left, grid or list on the right. Albums, playlists and
//! artists stand in three columns (Metrolist's grid); the tracks are one
//! long list loaded as it scrolls.

use egui::{Context, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::{BottomSheet, Button, ConnectedGroup, FilterChip, IconButton, Size, Style, TextInputDialog, TextInputResult};
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, LIKED_PLAYLIST, LibraryTab, ReleaseSort, Route, TrackSort, ViewState};

use crate::parts::{self, PageOut, Picture};
use crate::rows::{self, Look};
use crate::{PageFrame, text};

const TRACK_SORTS: [TrackSort; 5] = [TrackSort::Artist, TrackSort::Title, TrackSort::Album, TrackSort::Added, TrackSort::Duration];
const RELEASE_SORTS: [ReleaseSort; 4] = [ReleaseSort::Artist, ReleaseSort::Title, ReleaseSort::Year, ReleaseSort::Added];

fn track_sort_label(s: TrackSort) -> &'static str {
    match s {
        TrackSort::Artist => "По исполнителю",
        TrackSort::Title => "По названию",
        TrackSort::Album => "По альбому",
        TrackSort::Added => "Недавно добавленные",
        TrackSort::Duration => "По длительности",
    }
}

fn release_sort_label(s: ReleaseSort) -> &'static str {
    match s {
        ReleaseSort::Artist => "По исполнителю",
        ReleaseSort::Title => "По названию",
        ReleaseSort::Year => "По году",
        ReleaseSort::Added => "Недавно добавленные",
    }
}

/// The chips, in their order.
const CHIPS: [(LibraryTab, &str); 4] =
    [(LibraryTab::Playlists, "Плейлисты"), (LibraryTab::Albums, "Альбомы"), (LibraryTab::Artists, "Исполнители"), (LibraryTab::Tracks, "Треки")];

#[derive(Default)]
pub struct Library {
    pub chip: Option<LibraryTab>,
    list: bool,
    sorting: bool,
    naming: bool,
    name: String,
    /// The window of tracks last asked for.
    asked: Option<(usize, usize)>,
}

impl Library {
    /// Cards in a list instead of a grid (screenshot runs).
    pub fn set_list(&mut self, on: bool) {
        self.list = on;
    }

    /// A route asked for this chip (`All`: none).
    pub fn set_tab(&mut self, tab: LibraryTab) {
        self.chip = CHIPS.iter().any(|(t, _)| *t == tab).then_some(tab);
    }

    pub fn show(&mut self, ui: &mut Ui, frame: PageFrame, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
        let mut page = PageOut::default();
        let ctx = ui.ctx().clone();
        let lib = &state.library;
        let rect = frame.rect;
        // The search bar.
        let search = parts::search_rect(rect, frame.top);
        page.search = parts::search_bar(ui, search, Id::new("lib-search"));
        // The chips.
        let chips = Rect::from_min_size(pos2(rect.left(), search.bottom() + 8.0), vec2(rect.width(), 48.0));
        self.chips(ui, chips, out);
        // The chip's tools.
        let mut top = chips.bottom() + 4.0;
        if self.chip.is_some() {
            let row = Rect::from_min_size(pos2(rect.left() + 16.0, chips.bottom()), vec2(rect.width() - 32.0, 48.0));
            self.tools(ui, row, state);
            top = row.bottom() + 4.0;
        }
        let body = Rect::from_min_max(pos2(rect.left(), top), rect.max);
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("lib-body", self.chip.map(|c| c as u8))));
        bui.set_clip_rect(body.intersect(ui.clip_rect()));
        let here = Route::Library(self.chip.unwrap_or(LibraryTab::All));
        crate::scroll(("lib-scroll", self.chip.map(|c| c as u8)), frame.reset).show(&mut bui, |ui| {
            if !lib.loaded {
                parts::loading(ui);
                return;
            }
            match self.chip {
                None => self.overview(ui, state, &mut page),
                Some(LibraryTab::Playlists) => self.playlists(ui, state, &mut page),
                Some(LibraryTab::Albums) => self.albums(ui, &lib.releases, &mut page),
                Some(LibraryTab::Artists) => self.artists(ui, state, &mut page),
                Some(LibraryTab::Tracks) => self.tracks(ui, state, &here, out, &mut page),
                Some(_) => {}
            }
            ui.add_space(24.0);
        });
        self.sheets(&ctx, state, out);
        page
    }

    /// The sort or a new playlist on the left, grid or list on the right.
    fn tools(&mut self, ui: &mut Ui, row: Rect, state: &ViewState) {
        let lib = &state.library;
        let mut left = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("lib-tools"));
        match self.chip {
            Some(LibraryTab::Albums) => {
                if left.add(Button::new(release_sort_label(lib.release_sort)).icon(Icon::Sort).style(Style::Tonal).size(Size::S).id_salt("lib-sort")).clicked() {
                    self.sorting = true;
                }
            }
            Some(LibraryTab::Tracks) => {
                if left.add(Button::new(track_sort_label(lib.track_sort)).icon(Icon::Sort).style(Style::Tonal).size(Size::S).id_salt("lib-sort")).clicked() {
                    self.sorting = true;
                }
            }
            Some(LibraryTab::Playlists) => {
                if left.add(Button::new("Новый плейлист").icon(Icon::Add).style(Style::Tonal).size(Size::S).id_salt("lib-new-playlist")).clicked() {
                    self.name.clear();
                    self.naming = true;
                }
            }
            _ => {}
        }
        if !matches!(self.chip, Some(LibraryTab::Tracks)) {
            // Two icon buttons of the connected group, 40 dp each.
            let w = 2.0 * 40.0 + 2.0 + 8.0;
            let r = Rect::from_min_max(pos2(row.right() - w, row.top()), row.max);
            let mut rui = ui.new_child(UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt("lib-layouts"));
            let picked = ConnectedGroup::new("lib-layout", &["Сетка", "Список"]).icons(&[Icon::GridView, Icon::ViewList]).size(Size::S).show(&mut rui, usize::from(self.list));
            if let Some(i) = picked {
                self.list = i == 1;
            }
        }
    }

    fn chips(&mut self, ui: &mut Ui, rect: Rect, out: &mut Vec<Command>) {
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(0.0, 6.0))).id_salt("lib-chips"));
        cui.set_clip_rect(rect.intersect(ui.clip_rect()));
        use egui::containers::scroll_area::{ScrollBarVisibility, ScrollSource};
        egui::ScrollArea::horizontal()
            .id_salt("lib-chips-scroll")
            .scroll_source(ScrollSource::ALL)
            .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
            .show(&mut cui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if self.chip.is_some() && ui.add(IconButton::new(Icon::Close).size(nikgob_md3e::components::Size::Xs).id_salt("lib-chip-clear")).clicked() {
                        self.chip = None;
                    }
                    for (tab, label) in CHIPS {
                        let on = self.chip == Some(tab);
                        if ui.add(FilterChip::new(label, on).id_salt(("lib-chip", tab as u8))).clicked() {
                            self.chip = if on { None } else { Some(tab) };
                            self.asked = None;
                            if self.chip == Some(LibraryTab::Tracks) {
                                out.push(Command::LoadTracks { offset: 0, limit: 200, sort: TrackSort::default() });
                            }
                        }
                    }
                    ui.add_space(16.0);
                });
            });
    }

    fn overview(&mut self, ui: &mut Ui, state: &ViewState, page: &mut PageOut) {
        self.playlist_items(ui, state, page);
        let recent = if state.library.recently_added.is_empty() { &state.library.releases } else { &state.library.recently_added };
        if !recent.is_empty() {
            parts::section(ui, "Альбомы");
            let shown = &recent[..recent.len().min(6)];
            self.album_cards(ui, shown, page, false);
        }
    }

    fn playlist_items(&mut self, ui: &mut Ui, state: &ViewState, page: &mut PageOut) {
        for p in state.library.playlists.iter().filter(|p| p.id == LIKED_PLAYLIST || !p.pinned || p.track_count > 0) {
            let uri = p.cover.as_ref().map(|c| c.uri());
            if parts::item(ui, Id::new(("lib-pl", p.id)), uri.as_deref(), &p.name, &text::tracks(u64::from(p.track_count)), Picture::Square).clicked() {
                page.nav = Some(Route::Playlist(p.id));
            }
        }
    }

    fn playlists(&mut self, ui: &mut Ui, state: &ViewState, page: &mut PageOut) {
        if self.list {
            self.playlist_items(ui, state, page);
            return;
        }
        let pls = &state.library.playlists;
        parts::grid(ui, pls.len(), 3, |ui, i, r| {
            let p = &pls[i];
            let uri = p.cover.as_ref().map(|c| c.uri());
            if parts::card(ui, r, Id::new(("lib-pl-card", p.id)), uri.as_deref(), &p.name, &text::tracks(u64::from(p.track_count)), Picture::Square).clicked() {
                page.nav = Some(Route::Playlist(p.id));
            }
        });
    }

    fn albums(&mut self, ui: &mut Ui, releases: &[nikgob_view::ReleaseCard], page: &mut PageOut) {
        let list = self.list;
        self.album_cards(ui, releases, page, list);
    }

    fn album_cards(&mut self, ui: &mut Ui, releases: &[nikgob_view::ReleaseCard], page: &mut PageOut, list: bool) {
        let sub = |r: &nikgob_view::ReleaseCard| match r.year {
            Some(y) => format!("{} • {y}", r.artist),
            None => r.artist.clone(),
        };
        if list {
            for r in releases {
                let uri = r.cover.as_ref().map(|c| c.uri());
                if parts::item(ui, Id::new(("lib-al", r.id)), uri.as_deref(), &r.title, &sub(r), Picture::Square).clicked() {
                    page.nav = Some(Route::Release(r.id));
                }
            }
            return;
        }
        parts::grid(ui, releases.len(), 3, |ui, i, rect| {
            let r = &releases[i];
            let uri = r.cover.as_ref().map(|c| c.uri());
            if parts::card(ui, rect, Id::new(("lib-al-card", r.id)), uri.as_deref(), &r.title, &sub(r), Picture::Square).clicked() {
                page.nav = Some(Route::Release(r.id));
            }
        });
    }

    fn artists(&mut self, ui: &mut Ui, state: &ViewState, page: &mut PageOut) {
        let artists = &state.library.artists;
        let sub = |a: &nikgob_view::ArtistCard| if a.track_count > 0 { text::tracks(u64::from(a.track_count)) } else { String::new() };
        if self.list {
            for a in artists {
                let uri = a.image.as_ref().map(|c| c.uri());
                if parts::item(ui, Id::new(("lib-ar", a.id)), uri.as_deref(), &a.name, &sub(a), Picture::Round).clicked() {
                    page.nav = Some(Route::Artist(a.id));
                }
            }
            return;
        }
        parts::grid(ui, artists.len(), 3, |ui, i, r| {
            let a = &artists[i];
            let uri = a.image.as_ref().map(|c| c.uri());
            if parts::card(ui, r, Id::new(("lib-ar-card", a.id)), uri.as_deref(), &a.name, &sub(a), Picture::Round).clicked() {
                page.nav = Some(Route::Artist(a.id));
            }
        });
    }

    /// Every track, loaded a window at a time around what is in sight.
    fn tracks(&mut self, ui: &mut Ui, state: &ViewState, here: &Route, out: &mut Vec<Command>, page: &mut PageOut) {
        let lib = &state.library;
        let n = lib.track_count;
        let top = ui.cursor().top();
        let sight = rows::in_sight(ui.clip_rect(), top, rows::ROW_H, n);
        let loaded = lib.tracks_offset..lib.tracks_offset + lib.tracks.len();
        let covered = sight.is_empty() || (loaded.start <= sight.start && sight.end <= loaded.end);
        if !covered {
            let offset = sight.start.saturating_sub(60);
            let limit = sight.len() + 180;
            if self.asked != Some((offset, limit)) {
                self.asked = Some((offset, limit));
                out.push(Command::LoadTracks { offset, limit, sort: lib.track_sort });
            }
        }
        let ids: Vec<nikgob_view::TrackId> = lib.tracks.iter().map(|t| t.id).collect();
        rows::list(ui, "lib-tracks", n, rows::ROW_H, |ui, i, r| {
            let Some(t) = i.checked_sub(lib.tracks_offset).and_then(|k| lib.tracks.get(k)) else {
                // Not loaded yet: the row's place, quiet.
                let s = Theme::get(ui.ctx()).scheme;
                let c = Rect::from_min_size(pos2(r.left() + 16.0, r.center().y - 24.0), vec2(48.0, 48.0));
                ui.painter().rect_filled(c, 8.0, s.surface_container_high);
                return;
            };
            let look = Look::list(t, state);
            let o = rows::track(ui, r, Id::new(("lib-track", t.id)), t, look, state);
            if o.tap {
                out.push(if look.playing {
                    Command::TogglePlay
                } else {
                    Command::PlayTracks { tracks: ids.clone(), start: i - lib.tracks_offset, context: "библиотеки".into() }
                });
            }
            if o.play_next {
                out.push(Command::PlayNext(vec![t.id]));
            }
            if o.menu {
                page.menu = Some((t.clone(), here.clone()));
            }
        });
    }

    /// The sort sheet and the new playlist's name.
    fn sheets(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        if self.naming {
            let mut open = true;
            let r = TextInputDialog::new("lib-new-playlist", "Новый плейлист", "Название")
                .icon(Icon::PlaylistAdd)
                .buttons("Создать", "Отмена")
                .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned()))
                .show(ctx, &mut open, &mut self.name);
            if matches!(r, Some(TextInputResult::Confirmed)) {
                out.push(Command::CreatePlaylist(self.name.trim().to_owned()));
            }
            if r.is_some() || !open {
                self.naming = false;
            }
        }
        let tracks = self.chip == Some(LibraryTab::Tracks);
        let options: Vec<(&str, bool, Command)> = if tracks {
            TRACK_SORTS
                .into_iter()
                .map(|s| (track_sort_label(s), state.library.track_sort == s, Command::LoadTracks { offset: 0, limit: 200, sort: s }))
                .collect()
        } else {
            RELEASE_SORTS.into_iter().map(|s| (release_sort_label(s), state.library.release_sort == s, Command::SortReleases(s))).collect()
        };
        let screen = ctx.content_rect();
        let mut picked = None;
        BottomSheet::new("lib-sort-sheet").height(40.0 + options.len() as f32 * 56.0 + 24.0).max_width(screen.width()).show(ctx, &mut self.sorting, |ui| {
            let theme = Theme::get(ui.ctx());
            let s = theme.scheme;
            for (i, (label, on, _)) in options.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
                let style = if *on { TypeRole::BodyLarge.emphasized() } else { TypeRole::BodyLarge.style() };
                text::paint_line(ui, pos2(r.left() + 8.0, r.center().y - style.line_height * 0.5), *label, style, if *on { s.primary } else { s.on_surface }, r.width() - 48.0);
                if *on {
                    Icon::Check.widget(true, s.primary, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(24.0, 24.0)));
                }
                if resp.clicked() {
                    picked = Some(i);
                }
            }
        });
        if let Some(i) = picked
            && let Some((_, _, c)) = options.into_iter().nth(i)
        {
            out.push(c);
            self.asked = None;
            self.sorting = false;
        }
    }
}
