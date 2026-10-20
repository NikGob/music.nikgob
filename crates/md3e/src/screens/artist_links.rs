//! Getting from a track to each of its artists: links on every name of an
//! artist line, and «К исполнителю» in menus, which opens the artist
//! straight away when there is one and a small chooser when there are
//! several.

use std::ops::Range;

use egui::{Context, Id, Pos2, Rect, TextFormat, Ui, Vec2};
use nikgob_view::{ArtistLinks, Route, StreamArtistLinks, StreamPage};

use crate::{
    Icon,
    components::{Menu, MenuItem, link_label},
};

/// The artists of a line with the page each opens.
pub(crate) type Choices = Vec<(String, Route)>;

pub(crate) fn library(c: &ArtistLinks) -> Choices {
    c.linked().into_iter().map(|(n, id)| (n.to_owned(), Route::Artist(*id))).collect()
}

/// A service's artists: their page in the service.
pub(crate) fn stream(c: &StreamArtistLinks) -> Choices {
    c.linked()
        .into_iter()
        .map(|(n, id)| (n.to_owned(), Route::Stream(StreamPage::Artist(id.clone()))))
        .collect()
}

/// Links on each library artist of `line` (which starts with the credit's
/// text): the byte ranges and the page of each.
pub(crate) fn library_spans(c: &ArtistLinks, line: &str) -> (Vec<Range<usize>>, Vec<Route>) {
    c.ranges_in(line).into_iter().map(|(r, id)| (r, Route::Artist(*id))).unzip()
}

/// [`library_spans`] for a service's artists.
pub(crate) fn stream_spans(c: &StreamArtistLinks, line: &str) -> (Vec<Range<usize>>, Vec<Route>) {
    c.ranges_in(line)
        .into_iter()
        .map(|(r, id)| (r, Route::Stream(StreamPage::Artist(id.clone()))))
        .unzip()
}

/// A one-line artist label (player bar, Now Playing) with a link on each
/// name; returns the page of the name clicked.
pub(crate) fn label(ui: &mut Ui, id_salt: &str, line: &str, c: &ArtistLinks, format: TextFormat) -> Option<Route> {
    let (ranges, routes) = library_spans(c, line);
    let out = link_label(ui, id_salt, line, &ranges, format);
    out.clicked.and_then(|i| routes.get(i).cloned())
}

const CHOOSER: &str = "artist-chooser";

/// «К исполнителю» picked in a menu: the artist's page when there is one;
/// with several, a chooser opens at the pointer and `None` comes back
/// (its pick arrives through [`chooser`]).
pub(crate) fn go_to(ctx: &Context, choices: Choices) -> Option<Route> {
    match choices.len() {
        0 => None,
        1 => choices.into_iter().next().map(|(_, r)| r),
        _ => {
            let at = ctx.pointer_latest_pos().unwrap_or_else(|| ctx.content_rect().center());
            ctx.data_mut(|d| d.insert_temp(Id::new(CHOOSER), (true, at, choices)));
            None
        }
    }
}

/// The open chooser of [`go_to`]: a menu of the artists, each with its
/// name. Call once a frame; returns the page picked.
pub(crate) fn chooser(ctx: &Context) -> Option<Route> {
    let key = Id::new(CHOOSER);
    let (mut open, at, choices): (bool, Pos2, Choices) = ctx.data(|d| d.get_temp(key))?;
    let mut menu = Menu::new((CHOOSER, "menu"));
    for (name, _) in &choices {
        menu = menu.item(MenuItem::new(name.clone()).icon(Icon::Person));
    }
    let picked = menu.popup(ctx, Rect::from_min_size(at, Vec2::ZERO), &mut open);
    let route = picked.and_then(|i| choices.get(i).map(|(_, r)| r.clone()));
    if open {
        ctx.data_mut(|d| d.insert_temp(key, (open, at, choices)));
    } else {
        ctx.data_mut(|d| d.remove::<(bool, Pos2, Choices)>(key));
    }
    route
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::{ArtistId, Credit, CreditName};

    #[test]
    fn spans_open_each_artist() {
        let (a, b) = (ArtistId::new(), ArtistId::new());
        let c: ArtistLinks = Credit(vec![
            CreditName { name: "A".into(), id: Some(a), join: " feat. ".into() },
            CreditName { name: "B".into(), id: Some(b), join: String::new() },
        ]);
        let (ranges, routes) = library_spans(&c, "A feat. B · Album");
        assert_eq!(ranges, vec![0..1, 8..9]);
        assert_eq!(routes, vec![Route::Artist(a), Route::Artist(b)]);
        assert_eq!(library(&c).len(), 2);
        let ctx = Context::default();
        assert_eq!(go_to(&ctx, library(&Credit::single("A", Some(a)))), Some(Route::Artist(a)));
        assert_eq!(go_to(&ctx, library(&c)), None, "two: the chooser opens");
        assert_eq!(go_to(&ctx, Vec::new()), None);
    }
}
