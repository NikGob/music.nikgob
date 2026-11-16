//! «Где играть» as a sheet: this phone, then the owner's devices (those in
//! reach first). A device that plays its own music is taken over as a
//! remote by a tap; one that is free gets what plays here; ⇄ beside one
//! that plays moves the queue over it. This phone, while another device
//! plays, brings the music back here after a transfer, or lets the remote
//! go after an attach.

use egui::{Context, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{BottomSheet, IconButton, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, MeshKind, RemoteHow, Status, ViewState};

use crate::text;

const ROW_H: f32 = 72.0;

/// One place the music can play.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// `None`: this phone.
    pub node: Option<String>,
    pub name: String,
    pub kind: MeshKind,
    /// What plays there: «Артист — Трек».
    pub line: Option<String>,
    pub online: bool,
    pub playing: bool,
    pub selected: bool,
    /// It plays music of its own (a tap takes it over as a remote).
    pub plays: bool,
    /// ⇄ in its row.
    pub swap: bool,
}

fn track_line(artist: &str, title: &str) -> String {
    if artist.trim().is_empty() { title.to_owned() } else { format!("{artist} — {title}") }
}

/// This phone, then the paired devices, those in reach first.
pub fn places(state: &ViewState) -> Vec<Place> {
    let target = state.target.as_ref();
    let attached = target.is_some_and(|t| t.how == RemoteHow::Attached);
    let parked = state.parked.as_ref().filter(|_| target.is_some()).and_then(|p| p.current.as_ref());
    let mut out = vec![Place {
        node: None,
        name: "Этот телефон".to_owned(),
        kind: MeshKind::Phone,
        line: parked.map(|t| track_line(&t.artist, &t.title)),
        online: true,
        playing: target.is_none() && state.player.status == Status::Playing,
        selected: target.is_none(),
        plays: false,
        swap: attached,
    }];
    let mut nodes: Vec<_> = state.mesh.nodes.iter().collect();
    nodes.sort_by_key(|n| !n.online);
    for n in nodes {
        let is_target = target.is_some_and(|t| t.node_id == n.id);
        let plays = n.online && n.playing.as_ref().is_some_and(|p| !p.paused);
        out.push(Place {
            node: Some(n.id.clone()),
            name: n.name.clone(),
            kind: n.kind,
            line: n.playing.as_ref().filter(|_| n.online).map(|p| track_line(&p.artists.join(", "), &p.title)),
            online: n.online || is_target,
            playing: plays,
            selected: is_target,
            plays,
            swap: plays && (!is_target || attached),
        });
    }
    out
}

/// What a tap on a place does (`None`: nothing).
pub fn choice(p: &Place, state: &ViewState) -> Option<Command> {
    match &p.node {
        None => state.target.as_ref().map(|t| match t.how {
            RemoteHow::Attached => Command::OutputDetach,
            RemoteHow::Transferred => Command::OutputTransfer { node_id: None },
        }),
        Some(_) if !p.online || p.selected => None,
        Some(id) if p.plays => Some(Command::OutputAttach { node_id: id.clone() }),
        Some(id) => Some(Command::OutputTransfer { node_id: Some(id.clone()) }),
    }
}

#[derive(Default)]
pub struct OutputSheet {
    open: bool,
}

impl OutputSheet {
    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Draws the sheet; «Карта сети» asked: true.
    pub fn show(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) -> bool {
        let places = places(state);
        let screen = ctx.content_rect();
        let h = (40.0 + places.len() as f32 * ROW_H + 64.0 + 24.0).min(screen.height() * 0.8);
        let mut picked: Option<Command> = None;
        let mut map = false;
        BottomSheet::new("phone-output").height(h).max_width(screen.width()).show(ctx, &mut self.open, |ui| {
            for (i, p) in places.iter().enumerate() {
                let (tap, swap) = row(ui, i, p);
                if tap {
                    picked = choice(p, state);
                }
                if swap {
                    picked = Some(Command::OutputTransfer { node_id: p.node.clone() });
                }
            }
            if !state.mesh.nodes.is_empty() {
                ui.add_space(8.0);
                if ui.add(nikgob_md3e::components::Button::new("Карта сети").icon(Icon::SpeakerGroup).style(nikgob_md3e::components::Style::Text).id_salt("phone-output-map")).clicked() {
                    map = true;
                }
            }
        });
        if let Some(c) = picked {
            out.push(c);
            self.open = false;
        }
        if map {
            self.open = false;
        }
        map
    }
}

/// A place's row: its avatar, its name over what plays there, the check
/// of the chosen one or ⇄. (tapped, ⇄ tapped)
fn row(ui: &mut Ui, i: usize, p: &Place) -> (bool, bool) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let id = Id::new(("phone-output-row", i));
    let resp = ui.interact(r, id, Sense::click());
    let fill = if p.selected { s.secondary_container } else { egui::Color32::TRANSPARENT };
    paint::container(ui.painter(), r.shrink2(vec2(0.0, 4.0)), Radii::all(16.0), fill, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r.shrink2(vec2(0.0, 4.0)), Radii::all(16.0), s.on_surface, layer);
    let center = pos2(r.left() + 12.0 + 20.0, r.center().y);
    phone::device_avatar(ui, center, 40.0, &format!("out-{i}"), p.kind, p.online, p.playing);
    let x = center.x + 20.0 + 16.0;
    let right = r.right() - 56.0;
    let alpha = if p.online { 1.0 } else { 0.38 };
    let ts = TypeRole::BodyLarge.style();
    let ss = TypeRole::BodyMedium.style();
    let has_line = p.line.is_some();
    let y = r.center().y - (ts.line_height + if has_line { ss.line_height } else { 0.0 }) * 0.5;
    let on = if p.selected { s.on_secondary_container } else { s.on_surface };
    text::paint_line(ui, pos2(x, y), p.name.clone(), ts, on.gamma_multiply(alpha), right - x);
    if let Some(l) = &p.line {
        text::paint_line(ui, pos2(x, y + ts.line_height), l.clone(), ss, s.on_surface_variant.gamma_multiply(alpha), right - x);
    }
    let end = Rect::from_min_size(pos2(r.right() - 52.0, r.center().y - 24.0), vec2(48.0, 48.0));
    let mut swap = false;
    if p.swap {
        swap = crate::parts::put(ui, end, IconButton::new(Icon::SwapHoriz).id_salt(("phone-output-swap", i))).clicked();
    } else if p.selected {
        Icon::Check.widget(true, s.primary, 24.0).paint_at(ui, Rect::from_center_size(end.center(), vec2(24.0, 24.0)));
    }
    (resp.clicked() && !swap, swap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::{MeshNodeView, MeshPlaying, RemoteLink, RemoteTarget};

    fn node(id: &str, online: bool, playing: bool) -> MeshNodeView {
        MeshNodeView {
            id: id.into(),
            name: id.into(),
            online,
            playing: playing.then(|| MeshPlaying { title: "Song".into(), artists: vec!["A".into()], ..MeshPlaying::default() }),
            ..MeshNodeView::default()
        }
    }

    #[test]
    fn a_tap_takes_over_what_plays_and_moves_the_music_to_what_is_free() {
        let mut state = ViewState::default();
        state.mesh.nodes = vec![node("pi", true, true), node("tv", true, false), node("off", false, false)];
        let p = places(&state);
        assert_eq!(p[0].node, None);
        assert_eq!(choice(&p[0], &state), None, "this phone plays already");
        assert_eq!(choice(&p[1], &state), Some(Command::OutputAttach { node_id: "pi".into() }));
        assert_eq!(choice(&p[2], &state), Some(Command::OutputTransfer { node_id: Some("tv".into()) }));
        assert_eq!(choice(&p[3], &state), None, "out of reach");
        assert!(p[1].swap && !p[2].swap);
    }

    #[test]
    fn back_to_this_phone_as_the_remote_was_reached() {
        let mut state = ViewState::default();
        state.mesh.nodes = vec![node("pi", true, true)];
        state.target = Some(RemoteTarget {
            node_id: "pi".into(),
            name: "Pi".into(),
            kind: MeshKind::Pi,
            link: RemoteLink::default(),
            volume: 1.0,
            muted: false,
            how: RemoteHow::Attached,
        });
        let p = places(&state);
        assert_eq!(choice(&p[0], &state), Some(Command::OutputDetach));
        assert!(p[1].selected);
    }
}
