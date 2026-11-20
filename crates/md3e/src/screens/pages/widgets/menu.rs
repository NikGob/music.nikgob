use super::*;

/// The stable identity of a row that opens a submenu: the code finds the
/// submenu by this, never by its translated label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubId {
    /// «Файл» of a track's menu.
    File,
    /// «Таймер сна» of the player's menu.
    Sleep,
}

/// One row of a menu built from actions: an item standing for `A`, a
/// divider, a group label (not an item: nothing to pick), or a row that
/// opens a submenu of rows (one level).
#[derive(Clone, Debug)]
pub enum Row<A> {
    Item(A, MenuItem),
    Divider,
    Label(String),
    Sub(SubId, MenuItem, Vec<Row<A>>),
}

impl<A> Row<A> {
    /// The same row for another kind of action (a submenu's rows too).
    pub fn map<B>(self, f: &mut impl FnMut(A) -> B) -> Row<B> {
        match self {
            Row::Item(a, item) => Row::Item(f(a), item),
            Row::Divider => Row::Divider,
            Row::Label(l) => Row::Label(l),
            Row::Sub(id, item, rows) => Row::Sub(id, item, rows.into_iter().map(|r| r.map(&mut *f)).collect()),
        }
    }
}

/// The actions of `rows` in item order (what a picked index stands for),
/// a submenu's in its place.
pub fn row_acts<A: Copy>(rows: &[Row<A>]) -> Vec<A> {
    let mut acts = Vec::new();
    for r in rows {
        match r {
            Row::Item(a, _) => acts.push(*a),
            Row::Divider | Row::Label(_) => {}
            Row::Sub(_, _, rows) => acts.extend(row_acts(rows)),
        }
    }
    acts
}

/// The items of `rows` in item order with their actions, a submenu's in
/// its place.
#[cfg(test)]
pub fn row_items<A: Copy>(rows: &[Row<A>]) -> Vec<(A, &MenuItem)> {
    let mut items = Vec::new();
    for r in rows {
        match r {
            Row::Item(a, item) => items.push((*a, item)),
            Row::Divider | Row::Label(_) => {}
            Row::Sub(_, _, rows) => items.extend(row_items(rows)),
        }
    }
    items
}

/// The rows of the submenu `id` of `rows`.
#[cfg(test)]
pub fn sub_rows<'a, A>(rows: &'a [Row<A>], id: SubId) -> Option<&'a [Row<A>]> {
    rows.iter().find_map(|r| match r {
        Row::Sub(k, _, rows) if *k == id => Some(rows.as_slice()),
        _ => None,
    })
}

/// The rows of the submenu `id` of `rows`, to add to.
pub fn sub_rows_mut<'a, A>(rows: &'a mut [Row<A>], id: SubId) -> Option<&'a mut Vec<Row<A>>> {
    rows.iter_mut().find_map(|r| match r {
        Row::Sub(k, _, rows) if *k == id => Some(rows),
        _ => None,
    })
}

/// `rows` as a menu shows them: a submenu with no item is left out, the
/// one item of a submenu stands in the menu in its place — but «Файл»,
/// where a track's file items always are, even one.
pub fn shown_rows<A>(rows: Vec<Row<A>>) -> Vec<Row<A>> {
    let mut shown = Vec::with_capacity(rows.len());
    for r in rows {
        match r {
            Row::Sub(id, parent, rows) => {
                let rows = shown_rows(rows);
                match rows.iter().filter(|r| matches!(r, Row::Item(..))).count() {
                    0 => {}
                    1 if id != SubId::File => shown.extend(rows.into_iter().filter(|r| matches!(r, Row::Item(..)))),
                    _ => shown.push(Row::Sub(id, parent, rows)),
                }
            }
            r => shown.push(r),
        }
    }
    shown
}

/// A menu of `rows` ([`shown_rows`]) and what each of its item indices
/// stands for. A divider at the start, at the end or after another is left
/// out.
pub fn menu_of<A: Copy>(key: impl egui::AsId, rows: Vec<Row<A>>) -> (Menu, Vec<A>) {
    let rows = shown_rows(rows);
    let acts = row_acts(&rows);
    (fill(Menu::new(key), rows), acts)
}

fn fill<A>(mut menu: Menu, rows: Vec<Row<A>>) -> Menu {
    let mut after_divider = true;
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        match r {
            Row::Item(_, item) => {
                menu = menu.item(item);
                after_divider = false;
            }
            Row::Sub(_, item, rows) => {
                menu = menu.item(item.submenu(fill(Menu::new("submenu"), rows)));
                after_divider = false;
            }
            Row::Label(text) => {
                menu = menu.group_label(text);
                after_divider = false;
            }
            Row::Divider => {
                if !after_divider && i + 1 < n {
                    menu = menu.divider();
                    after_divider = true;
                }
            }
        }
    }
    menu
}

