//! «Межсеть → Устройства» as the loop keeps it: `[mesh]` of settings.toml
//! (the switch, this device's name), the node table (the paired devices,
//! names given here, what each stores), a device being forgotten while its
//! snackbar's «Отменить» is up. The network is the mesh service's: what
//! needs it is passed on ([`MeshKeep::link`]), and what is live comes back
//! as [`Command::MeshUpdate`] and is merged here. Nothing here waits on the
//! network.

use std::collections::HashMap;
use std::sync::Arc;

use nikgob_core::settings::{MeshPinsRecord, MeshSettings, Settings};
use nikgob_library::{NodeRow, OnlyCopy};
use nikgob_view::{Command, MeshForgetAsk, MeshKind, MeshNodeView, MeshPairing, MeshPins, MeshProfile, MeshScope, MeshSnapshot, MeshSyncView, MeshView};

use crate::{Later, Worker};

/// A device being forgotten goes for good no sooner than this after the
/// click (its snackbar gone).
const FORGET_AFTER_MS: i64 = 5_000;
/// A device's last-seen time is written at most this often.
const SEEN_SAVE_MS: i64 = 60_000;

/// What the loop keeps of the mesh between turns.
#[derive(Default)]
pub(crate) struct MeshKeep {
    /// Passes a command on to the mesh service (set when it starts).
    pub(crate) link: Option<Arc<dyn Fn(Command) + Send + Sync>>,
    settings: MeshSettings,
    configurable: bool,
    rows: Vec<NodeRow>,
    /// The service's last report, once it made one.
    live: Option<MeshSnapshot>,
    /// Devices forgotten here, their «Отменить» up: (id, when).
    forgetting: Vec<(String, i64)>,
    /// Forgets passed to the mesh service: hidden until a report no longer
    /// lists the device (its row goes then) or the service says it failed.
    sent: Vec<String>,
    /// A pairing that could not even start here (no mesh service).
    failed: Option<String>,
    /// Devices whose forget is being checked (the originals only they hold)
    /// on a thread.
    checking: Vec<String>,
    /// A forget the check stopped, waiting for the owner.
    ask: Option<MeshForgetAsk>,
    /// Devices whose transfer is paused here.
    paused: Vec<String>,
    /// The devices the library's sync is turned off with.
    sync_off: Vec<String>,
    /// The sync loop's last report per device.
    sync: HashMap<String, MeshSyncView>,
}

/// The pins as the view shows them.
fn pins_view(r: Option<&MeshPinsRecord>) -> MeshPins {
    r.map(|r| MeshPins { liked: r.liked, playlists: r.playlists.clone() }).unwrap_or_default()
}

/// `pins` of device `node` laid into `all` (a record replaced, added, or
/// dropped when it keeps nothing). True when it changed anything.
pub(crate) fn set_pins(all: &mut Vec<MeshPinsRecord>, node: &str, pins: &MeshPins) -> bool {
    let record = MeshPinsRecord { node: node.to_owned(), liked: pins.liked, playlists: pins.playlists.clone() };
    let empty = !pins.liked && pins.playlists.is_empty();
    match all.iter().position(|p| p.node == node) {
        Some(i) if empty => {
            all.remove(i);
            true
        }
        Some(i) if all[i] != record => {
            all[i] = record;
            true
        }
        Some(_) => false,
        None if empty => false,
        None => {
            all.push(record);
            true
        }
    }
}

impl MeshKeep {
    /// The sync with `id` as the view shows it.
    fn sync_of(&self, id: &str) -> MeshSyncView {
        let mut s = self.sync.get(id).cloned().unwrap_or_default();
        s.off = self.sync_off.iter().any(|o| o == id);
        if s.off {
            s.running = false;
        }
        s
    }
}

fn row_view(r: &NodeRow) -> MeshNodeView {
    MeshNodeView {
        id: r.id.clone(),
        name: r.shown_name().to_owned(),
        kind: MeshKind::parse(&r.kind).unwrap_or_default(),
        scope: MeshScope::parse(&r.scope).unwrap_or_default(),
        online: false,
        last_seen_ms: r.last_seen,
        addr: r.addr.clone(),
        version: r.version.clone(),
        profile: MeshProfile::parse(&r.profile).unwrap_or_default(),
        ..MeshNodeView::default()
    }
}

fn with_sync(keep: &MeshKeep, mut v: MeshNodeView) -> MeshNodeView {
    v.sync = keep.sync_of(&v.id);
    v
}

/// The view: the service's report (when there is one) with what is kept
/// here over it — names given here, profiles, the devices being forgotten
/// left out; without a report, the devices as kept, offline.
pub(crate) fn merge(keep: &MeshKeep) -> MeshView {
    let hidden = |id: &str| keep.forgetting.iter().any(|(f, _)| f == id) || keep.sent.iter().any(|f| f == id);
    let mut nodes: Vec<MeshNodeView> = match &keep.live {
        Some(live) => live
            .nodes
            .iter()
            .filter(|n| !hidden(&n.view.id))
            .map(|n| {
                let mut v = n.view.clone();
                if let Some(r) = keep.rows.iter().find(|r| r.id == v.id) {
                    if let Some(a) = r.alias.as_deref().filter(|a| !a.trim().is_empty()) {
                        v.name = a.to_owned();
                    }
                    v.profile = MeshProfile::parse(&r.profile).unwrap_or_default();
                    if v.addr.is_none() {
                        v.addr = r.addr.clone();
                    }
                    if v.last_seen_ms.is_none() {
                        v.last_seen_ms = r.last_seen;
                    }
                    if v.version.is_none() {
                        v.version = r.version.clone();
                    }
                }
                with_sync(keep, v)
            })
            .collect(),
        None => keep.rows.iter().filter(|r| !hidden(&r.id)).map(|r| with_sync(keep, row_view(r))).collect(),
    };
    for n in &mut nodes {
        n.pins = pins_view(keep.settings.pins_of(&n.id));
        if let Some(t) = n.transfer.as_mut() {
            t.paused = keep.paused.contains(&n.id);
        }
    }
    nodes.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    let live = keep.live.clone().unwrap_or_default();
    let pairing = match (&keep.failed, live.pairing) {
        (Some(why), MeshPairing::Idle) => MeshPairing::Failed(why.clone()),
        (_, p) => p,
    };
    MeshView {
        enabled: keep.settings.enabled,
        running: live.running,
        this: live.this,
        nodes,
        nearby: live.nearby,
        pairing,
        invite: live.invite,
        ring_holder: live.ring_holder,
        paired: live.paired,
        code: live.code,
        error: live.error,
        name_setting: keep.settings.name.clone(),
        configurable: keep.configurable,
        keep_masters: keep.settings.keep_masters,
        forget_ask: keep.ask.clone(),
    }
}

/// How many of `only` (originals one device alone holds a verified copy
/// of) this computer no longer has either: kept by name and that file gone,
/// kept flat and no `{hash}.<ext>` among `flat` (the stems of the masters
/// folder's files). `exists` says whether a path is a file.
pub(crate) fn lost_with(only: &[OnlyCopy], flat: &std::collections::HashSet<String>, exists: impl Fn(&str) -> bool) -> u64 {
    only.iter().filter(|o| !(o.paths.iter().any(|p| exists(p.as_str())) || (o.flat && flat.contains(&o.file_hash)))).count() as u64
}

/// The stems of the files right in the masters folder (`{hash}.flac`, not
/// the `.json` sidecars, not the cue sheets).
fn flat_masters(dir: &std::path::Path) -> std::collections::HashSet<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Default::default() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("cue")))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect()
}

/// The originals forgetting `node` would lose: those only it holds a
/// verified copy of among the trusted devices staying (`leaving` go too),
/// that this computer does not have either. Reads the library and the
/// masters folder: never on the loop.
fn lost_if_forgotten(lib: &nikgob_library::Library, node: &str, leaving: &[String], masters_dir: &std::path::Path) -> Result<u64, String> {
    let only = lib.flac_only_on(node, leaving).map_err(|e| e.to_string())?;
    if only.is_empty() {
        return Ok(0);
    }
    let flat = if only.iter().any(|o| o.flat) { flat_masters(masters_dir) } else { Default::default() };
    Ok(lost_with(&only, &flat, |p| std::path::Path::new(p).is_file()))
}

/// The rows a report brings: new devices, and kept ones whose own facts
/// changed (the last-seen time at most once a minute). `now` is unix ms.
pub(crate) fn rows_to_write(live: &MeshSnapshot, rows: &[NodeRow], now: i64) -> Vec<NodeRow> {
    let mut out = Vec::new();
    for n in &live.nodes {
        let v = &n.view;
        let old = rows.iter().find(|r| r.id == v.id);
        let row = NodeRow {
            id: v.id.clone(),
            name: v.name.clone(),
            alias: old.and_then(|o| o.alias.clone()),
            kind: v.kind.key().to_owned(),
            scope: v.scope.key().to_owned(),
            profile: old.map_or_else(|| MeshProfile::Full.key(), |o| o.profile.clone()),
            last_seen: if v.online { Some(now) } else { v.last_seen_ms.or(old.and_then(|o| o.last_seen)) },
            addr: v.addr.clone().or_else(|| old.and_then(|o| o.addr.clone())),
            version: v.version.clone().or_else(|| old.and_then(|o| o.version.clone())),
            cert: Some(n.cert.clone()).filter(|c| !c.is_empty()).or_else(|| old.and_then(|o| o.cert.clone())),
            paired_at: old.and_then(|o| o.paired_at).or(Some(now)),
        };
        let changed = match old {
            None => true,
            Some(o) => {
                o.name != row.name
                    || o.kind != row.kind
                    || o.scope != row.scope
                    || o.addr != row.addr
                    || o.version != row.version
                    || o.cert != row.cert
                    || row.last_seen.unwrap_or(0) - o.last_seen.unwrap_or(0) >= SEEN_SAVE_MS
            }
        };
        if changed {
            out.push(row);
        }
    }
    out
}

/// The kept devices no longer in the ring (forgotten from another device):
/// only a report of a device in a ring says so.
pub(crate) fn rows_gone(live: &MeshSnapshot, rows: &[NodeRow], forgetting: &[(String, i64)]) -> Vec<String> {
    if !live.paired {
        return Vec::new();
    }
    rows.iter()
        .filter(|r| !live.nodes.iter().any(|n| n.view.id == r.id) && !forgetting.iter().any(|(f, _)| *f == r.id))
        .map(|r| r.id.clone())
        .collect()
}

impl Worker {
    /// `[mesh]` and the kept devices into the view at start.
    pub(crate) fn mesh_load(&mut self) {
        if let Some(path) = self.settings_path.clone() {
            match Settings::load(&path) {
                Ok(s) => {
                    self.mesh.settings = s.mesh;
                    self.mesh.configurable = true;
                }
                Err(e) => log::warn!("mesh: settings.toml not read: {e}"),
            }
        }
        match self.lib.nodes() {
            Ok(rows) => self.mesh.rows = rows,
            Err(e) => log::warn!("mesh: the paired devices not read: {e}"),
        }
        match self.lib.sync_off_nodes() {
            Ok(off) => self.mesh.sync_off = off,
            Err(e) => log::warn!("mesh: the devices sync is off with not read: {e}"),
        }
        self.mesh_publish();
    }

    /// «Забыть»: first, on a thread, whether originals would go with the
    /// device (a fresh look at the library and the masters folder); the
    /// forget starts only on a clean answer, else it waits for the owner.
    fn mesh_check_forget(&mut self, node_id: String) {
        let known = self.state.read().mesh.node(&node_id).is_some();
        if !known || self.mesh.forgetting.iter().any(|(f, _)| *f == node_id) || self.mesh.checking.contains(&node_id) {
            return;
        }
        let leaving: Vec<String> = self.mesh.forgetting.iter().map(|(f, _)| f.clone()).chain(self.mesh.sent.iter().cloned()).collect();
        let masters_dir = self.state.read().prefs.masters_dir.clone();
        let lib = self.lib.clone();
        let later = self.later_tx.clone();
        self.mesh.checking.push(node_id.clone());
        let id = node_id.clone();
        let spawned = std::thread::Builder::new().name("nikgob-forget-check".into()).spawn(move || {
            let lost = lost_if_forgotten(&lib, &id, &leaving, &masters_dir);
            let _ = later.send(Later::new("a forget's originals", move |w| w.mesh_forget_checked(id, lost)));
        });
        if let Err(e) = spawned {
            self.mesh_forget_checked(node_id, Err(e.to_string()));
        }
    }

    /// What the check found: nothing to lose — the forget starts; some, or
    /// a check that failed — the owner is asked.
    fn mesh_forget_checked(&mut self, node_id: String, lost: Result<u64, String>) {
        self.mesh.checking.retain(|c| *c != node_id);
        let name = self.state.read().mesh.node(&node_id).map(|n| n.name.clone());
        let Some(name) = name else { return };
        match lost {
            Ok(0) => self.mesh_start_forget(node_id),
            Ok(n) => {
                self.mesh.ask = Some(MeshForgetAsk { node_id, name, originals: Some(n) });
                self.mesh_publish();
            }
            Err(e) => {
                log::warn!("mesh: the originals of a device being forgotten not checked: {e}");
                self.mesh.ask = Some(MeshForgetAsk { node_id, name, originals: None });
                self.mesh_publish();
            }
        }
    }

    /// The forget itself: hidden at once, its snackbar's «Отменить» up, for
    /// good when it is gone.
    fn mesh_start_forget(&mut self, node_id: String) {
        let name = {
            let st = self.state.read();
            st.mesh.node(&node_id).map(|n| n.name.clone())
        };
        let Some(name) = name else { return };
        if self.mesh.forgetting.iter().any(|(f, _)| *f == node_id) {
            return;
        }
        self.mesh.forgetting.push((node_id.clone(), nikgob_library::now_ms()));
        self.mesh_publish();
        self.toast(format!("Устройство «{name}» забыто"), Some(("Отменить".to_owned(), Command::MeshRestore { node_id })));
    }

    fn mesh_publish(&mut self) {
        let view = merge(&self.mesh);
        if self.state.read().mesh != view {
            self.publish(move |s| s.mesh = view);
            // A device driven from here that is gone from the list.
            self.connect_check_target();
        }
    }

    /// Changes `[mesh]` in settings.toml; a failure is a toast.
    fn mesh_edit(&mut self, edit: impl FnOnce(&mut MeshSettings) -> bool) {
        let Some(path) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return;
        };
        let mut settings = match Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
                return;
            }
        };
        if !edit(&mut settings.mesh) {
            return;
        }
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast("Не удалось сохранить настройки", None);
            return;
        }
        self.mesh.settings = settings.mesh;
        self.mesh.configurable = true;
        self.mesh_publish();
    }

    /// Writes changed rows into the node table and the kept list.
    fn mesh_put(&mut self, rows: Vec<NodeRow>) {
        if rows.is_empty() {
            return;
        }
        if let Err(e) = self.lib.put_nodes(&rows) {
            log::error!("mesh: the paired devices not saved: {e}");
        }
        for r in rows {
            match self.mesh.rows.iter_mut().find(|x| x.id == r.id) {
                Some(x) => *x = r,
                None => self.mesh.rows.push(r),
            }
        }
    }

    fn mesh_remove(&mut self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        if let Err(e) = self.lib.remove_nodes(ids) {
            log::error!("mesh: the forgotten devices not removed: {e}");
        }
        self.mesh.rows.retain(|r| !ids.contains(&r.id));
        self.mesh.paused.retain(|p| !ids.contains(p));
        if self.mesh.ask.as_ref().is_some_and(|a| ids.contains(&a.node_id)) {
            self.mesh.ask = None;
        }
        // What a forgotten device pinned goes with it.
        if self.mesh.settings.pins.iter().any(|p| ids.contains(&p.node)) {
            let gone = ids.to_vec();
            self.mesh_edit(move |m| {
                let before = m.pins.len();
                m.pins.retain(|p| !gone.contains(&p.node));
                m.pins.len() != before
            });
        }
    }

    /// Passes a command that needs the network on to the mesh service.
    fn mesh_forward(&mut self, c: Command) {
        match self.mesh.link.clone() {
            Some(f) => {
                self.mesh.failed = None;
                f(c);
            }
            None => {
                if matches!(c, Command::MeshPairNearby { .. } | Command::MeshCreateInvite { .. } | Command::MeshJoin { .. }) {
                    self.mesh.failed = Some("Межсеть не запущена".to_owned());
                    self.mesh_publish();
                } else if c == Command::MeshClearPairing && self.mesh.failed.take().is_some() {
                    self.mesh_publish();
                }
            }
        }
    }

    pub(crate) fn mesh_command(&mut self, c: Command) {
        match c {
            Command::MeshSetEnabled(on) => self.mesh_edit(|m| {
                let changed = m.enabled != on;
                m.enabled = on;
                changed
            }),
            Command::MeshRename { node_id: None, name } => {
                let name = name.trim().to_owned();
                self.mesh_edit(move |m| {
                    let changed = m.name != name;
                    m.name = name;
                    changed
                });
            }
            Command::MeshRename { node_id: Some(id), name } => {
                let Some(mut row) = self.mesh.rows.iter().find(|r| r.id == id).cloned() else { return };
                let name = name.trim();
                row.alias = (!name.is_empty() && name != row.name).then(|| name.to_owned());
                self.mesh_put(vec![row]);
                self.mesh_publish();
            }
            Command::MeshSetProfile { node_id, profile } => {
                let Some(mut row) = self.mesh.rows.iter().find(|r| r.id == node_id).cloned() else { return };
                row.profile = profile.key();
                self.mesh_put(vec![row]);
                self.mesh_publish();
            }
            Command::MeshForget { node_id } => self.mesh_check_forget(node_id),
            Command::MeshForgetAnswer { node_id, forget } => {
                if self.mesh.ask.as_ref().is_some_and(|a| a.node_id == node_id) {
                    self.mesh.ask = None;
                    self.mesh_publish();
                    if forget {
                        self.mesh_start_forget(node_id);
                    }
                }
            }
            Command::MeshRestore { node_id } => {
                self.mesh.forgetting.retain(|(f, _)| *f != node_id);
                self.mesh_publish();
            }
            Command::MeshUpdate(snap) => {
                let now = nikgob_library::now_ms();
                let write = rows_to_write(&snap, &self.mesh.rows, now);
                let gone = rows_gone(&snap, &self.mesh.rows, &self.mesh.forgetting);
                self.mesh_put(write);
                self.mesh_remove(&gone);
                self.mesh.sent.retain(|id| snap.nodes.iter().any(|n| n.view.id == *id));
                if snap.pairing != MeshPairing::Idle {
                    self.mesh.failed = None;
                }
                self.mesh.live = Some(*snap);
                self.mesh_publish();
            }
            Command::MeshSetKeepMasters(keep) => {
                self.mesh_edit(|m| {
                    let changed = m.keep_masters != keep;
                    m.keep_masters = keep;
                    changed
                });
                self.mesh_publish();
            }
            Command::MeshSetPins { node_id, pins } => self.mesh_edit(move |m| set_pins(&mut m.pins, &node_id, &pins)),
            Command::MeshPauseTransfer { node_id, paused } => {
                self.mesh.paused.retain(|p| *p != node_id);
                if paused {
                    self.mesh.paused.push(node_id.clone());
                }
                self.mesh_publish();
                // The files' side (the replicas) hears it through the mesh.
                self.mesh_forward(Command::MeshPauseTransfer { node_id, paused });
            }
            Command::MeshForgetFailed { node_id, why } => {
                self.mesh.sent.retain(|f| *f != node_id);
                let name = self.mesh.live.as_ref().and_then(|l| l.nodes.iter().find(|n| n.view.id == node_id)).map(|n| {
                    self.mesh.rows.iter().find(|r| r.id == node_id).map_or_else(|| n.view.name.clone(), |r| r.shown_name().to_owned())
                });
                self.mesh_publish();
                self.toast(format!("Не удалось забыть «{}»: {why}", name.unwrap_or_else(|| "устройство".to_owned())), None);
            }
            Command::MeshSetSync { node_id, on } => {
                if let Err(e) = self.lib.set_sync_off(&node_id, !on) {
                    log::error!("mesh: the sync switch not saved: {e}");
                    self.toast("Не удалось сохранить настройку", None);
                    return;
                }
                self.mesh.sync_off.retain(|o| *o != node_id);
                if !on {
                    self.mesh.sync_off.push(node_id);
                }
                self.mesh_publish();
            }
            Command::MeshSyncReport { node_id, sync } => {
                if self.mesh.sync.get(&node_id) != Some(&sync) {
                    self.mesh.sync.insert(node_id, sync);
                    self.mesh_publish();
                }
            }
            c @ (Command::MeshPairNearby { .. }
            | Command::MeshCreateInvite { .. }
            | Command::MeshCancelInvite
            | Command::MeshJoin { .. }
            | Command::MeshSetScope { .. }
            | Command::MeshClearPairing) => self.mesh_forward(c),
            other => log::warn!("mesh: not a mesh command {other:?}"),
        }
    }

    /// The loop's tick: a device whose «Отменить» is gone is forgotten for
    /// good (its row goes; the ring holder revokes it).
    pub(crate) fn mesh_tick(&mut self) {
        let now = nikgob_library::now_ms();
        if self.mesh.forgetting.is_empty() {
            return;
        }
        let due: Vec<String> = {
            let st = self.state.read();
            self.mesh
                .forgetting
                .iter()
                .filter(|(id, at)| {
                    let up = st.toasts.iter().any(|t| matches!(&t.action, Some((_, Command::MeshRestore { node_id })) if node_id == id));
                    now - at >= FORGET_AFTER_MS && !up
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        if due.is_empty() {
            return;
        }
        self.mesh.forgetting.retain(|(f, _)| !due.contains(f));
        self.mesh_send_forgets(due);
        self.mesh_publish();
    }

    /// Passes forgets to the mesh service; their rows go when its report no
    /// longer lists them. Without a service there is no ring to tell: the
    /// rows go now.
    fn mesh_send_forgets(&mut self, ids: Vec<String>) {
        if self.mesh.link.is_none() {
            self.mesh_remove(&ids);
            return;
        }
        for id in ids {
            self.mesh.sent.push(id.clone());
            self.mesh_forward(Command::MeshForget { node_id: id });
        }
    }

    /// The mesh is about to stop ([`crate::RuntimeHandle::flush_mesh`]): the
    /// devices still being forgotten go to it now (their «Отменить» ends
    /// with the run), so it revokes them before it stops.
    pub(crate) fn mesh_flush_now(&mut self) {
        let due: Vec<String> = self.mesh.forgetting.drain(..).map(|(id, _)| id).collect();
        if due.is_empty() {
            return;
        }
        self.mesh_send_forgets(due);
        self.mesh_publish();
    }

    /// At the end: what is left of the forgets (no mesh, or it stopped
    /// before) — the rows go, the command is passed on if anyone listens.
    pub(crate) fn mesh_flush(&mut self) {
        let due: Vec<String> = self.mesh.forgetting.drain(..).map(|(id, _)| id).collect();
        self.mesh_remove(&due);
        for id in due {
            self.mesh_forward(Command::MeshForget { node_id: id });
        }
    }
}

#[cfg(test)]
mod tests {
    use nikgob_view::MeshSnapshotNode;

    use super::*;

    fn node(id: &str, name: &str, online: bool) -> MeshSnapshotNode {
        MeshSnapshotNode {
            view: MeshNodeView { id: id.into(), name: name.into(), kind: MeshKind::Pi, online, addr: Some("192.168.0.157:47220".into()), ..MeshNodeView::default() },
            cert: "{\"node\":\"x\"}".into(),
        }
    }

    fn row(id: &str, name: &str) -> NodeRow {
        NodeRow { id: id.into(), name: name.into(), kind: "pi".into(), scope: "owner".into(), profile: "full".into(), ..NodeRow::default() }
    }

    #[test]
    fn without_a_report_the_kept_devices_show_offline() {
        let keep = MeshKeep { rows: vec![row("a", "Pi")], settings: MeshSettings::default(), ..MeshKeep::default() };
        let v = merge(&keep);
        assert!(v.enabled);
        assert!(!v.running);
        assert_eq!(v.nodes.len(), 1);
        assert!(!v.nodes[0].online);
        assert_eq!(v.nodes[0].kind, MeshKind::Pi);
    }

    #[test]
    fn the_report_shows_with_names_and_profiles_kept_here() {
        let mut r = row("a", "Raspberry Pi");
        r.alias = Some("Кухня".into());
        r.profile = "library".into();
        let live = MeshSnapshot { running: true, paired: true, nodes: vec![node("a", "Raspberry Pi", true), node("b", "TV", false)], ..MeshSnapshot::default() };
        let keep = MeshKeep { rows: vec![r], live: Some(live), forgetting: vec![("b".into(), 0)], ..MeshKeep::default() };
        let v = merge(&keep);
        assert_eq!(v.nodes.len(), 1, "the one being forgotten is left out");
        assert_eq!(v.nodes[0].name, "Кухня");
        assert_eq!(v.nodes[0].profile, MeshProfile::Library);
        assert!(v.nodes[0].online);
    }

    /// A forget passed to the mesh stays hidden while reports still list the
    /// device (the ring has not taken it yet).
    #[test]
    fn a_forget_passed_on_stays_hidden() {
        let live = MeshSnapshot { running: true, paired: true, nodes: vec![node("a", "Pi", true), node("b", "TV", true)], ..MeshSnapshot::default() };
        let keep = MeshKeep { rows: vec![row("a", "Pi"), row("b", "TV")], live: Some(live.clone()), sent: vec!["b".into()], ..MeshKeep::default() };
        let v = merge(&keep);
        assert_eq!(v.nodes.len(), 1);
        assert_eq!(v.nodes[0].id, "a");
        // The report without it: its row goes (not kept back like one with
        // its snackbar still up).
        let after = MeshSnapshot { nodes: vec![node("a", "Pi", true)], ..live };
        assert_eq!(rows_gone(&after, &keep.rows, &[]), vec!["b".to_owned()]);
    }

    #[test]
    fn a_report_writes_new_and_changed_devices_and_the_last_seen_rarely() {
        let live = MeshSnapshot { paired: true, nodes: vec![node("a", "Pi", true)], ..MeshSnapshot::default() };
        let first = rows_to_write(&live, &[], 1_000_000);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].paired_at, Some(1_000_000));
        assert_eq!(first[0].last_seen, Some(1_000_000));
        let kept = first.clone();
        assert!(rows_to_write(&live, &kept, 1_030_000).is_empty(), "half a minute later: nothing");
        assert_eq!(rows_to_write(&live, &kept, 1_061_000).len(), 1, "a minute later: the last seen");
        let renamed = MeshSnapshot { paired: true, nodes: vec![node("a", "Кухня", true)], ..MeshSnapshot::default() };
        let w = rows_to_write(&renamed, &kept, 1_001_000);
        assert_eq!(w[0].name, "Кухня");
        assert_eq!(w[0].paired_at, Some(1_000_000), "paired once");
    }

    #[test]
    fn devices_leave_the_table_only_when_a_ring_says_so() {
        let rows = vec![row("a", "Pi"), row("b", "TV"), row("c", "Phone")];
        let live = MeshSnapshot { paired: true, nodes: vec![node("a", "Pi", true)], ..MeshSnapshot::default() };
        assert_eq!(rows_gone(&live, &rows, &[("c".into(), 0)]), vec!["b".to_owned()]);
        let unpaired = MeshSnapshot { paired: false, ..MeshSnapshot::default() };
        assert!(rows_gone(&unpaired, &rows, &[]).is_empty());
    }

    /// The view carries each device's pins, a transfer's pause, the choice
    /// of the originals and a forget waiting for the owner.
    #[test]
    fn pins_pauses_and_a_forget_asked_show_in_the_view() {
        let mut settings = MeshSettings { keep_masters: nikgob_core::settings::KeepMasters::AfterOne, ..MeshSettings::default() };
        let list = nikgob_view::PlaylistId::new();
        assert!(set_pins(&mut settings.pins, "a", &MeshPins { liked: true, playlists: vec![list] }));
        let mut live_a = node("a", "Pi", true);
        live_a.view.transfer = Some(nikgob_view::MeshTransfer { done: 1, total: 2, paused: false, speed: 0 });
        let live = MeshSnapshot { running: true, paired: true, nodes: vec![live_a, node("b", "TV", true)], ..MeshSnapshot::default() };
        let ask = MeshForgetAsk { node_id: "a".into(), name: "Pi".into(), originals: Some(4) };
        let keep = MeshKeep {
            rows: vec![row("a", "Pi"), row("b", "TV")],
            live: Some(live),
            settings,
            ask: Some(ask.clone()),
            paused: vec!["a".into()],
            ..MeshKeep::default()
        };
        let v = merge(&keep);
        assert_eq!(v.keep_masters, nikgob_core::settings::KeepMasters::AfterOne);
        assert_eq!(v.forget_ask, Some(ask));
        let a = v.node("a").unwrap();
        assert_eq!(a.pins, MeshPins { liked: true, playlists: vec![list] });
        assert!(a.transfer.as_ref().unwrap().paused);
        assert_eq!(v.node("b").unwrap().pins, MeshPins::default());
    }

    /// An original only the device holds is lost with it unless this
    /// computer still has its file: by name, that path; flat, its hash in
    /// the masters folder.
    #[test]
    fn an_original_is_lost_only_when_this_computer_lacks_it_too() {
        let only = |hash: &str, paths: &[&str], flat: bool| OnlyCopy { file_hash: hash.into(), paths: paths.iter().map(|p| (*p).to_owned()).collect(), flat };
        let list = [
            only("h1", &[], true),
            only("h2", &[], true),
            only("h3", &["/m/a.flac"], false),
            only("h4", &["/m/gone.flac"], false),
            only("h5", &["/m/gone.flac"], true),
        ];
        let flat: std::collections::HashSet<String> = ["h1".to_owned(), "h5".to_owned()].into();
        let exists = |p: &str| p == "/m/a.flac";
        // h2 (flat, not there) and h4 (by name, gone) go; h5 is still flat.
        assert_eq!(lost_with(&list, &flat, exists), 2);
        assert_eq!(lost_with(&[], &flat, exists), 0);
    }

    /// The check over a real library and masters folder: of two originals
    /// only the Pi holds, the one whose file this computer still has does
    /// not count; the sidecar of the other is no file of it.
    #[test]
    fn a_forget_checks_the_library_and_the_masters_folder() {
        use nikgob_core::model::{Codec, DeviceId, Master, Replica, TrackId};
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        lib.put_nodes(&[row("pi", "Pi")]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("h1.flac"), b"x").unwrap();
        std::fs::write(dir.path().join("h2.json"), b"{}").unwrap();
        for h in ["h1", "h2"] {
            let replica = Replica { file_hash: h.into(), node_id: DeviceId("pi".into()), verified_at: Some(1) };
            let m = Master {
                track_id: TrackId::new(),
                codec: Codec::Flac,
                file_hash: h.into(),
                size_bytes: 10,
                origin: None,
                file_path: None,
                replicas: vec![replica],
            };
            lib.put_master(&m).unwrap();
        }
        assert_eq!(lost_if_forgotten(&lib, "pi", &[], dir.path()), Ok(1), "h2 only");
        assert_eq!(lost_if_forgotten(&lib, "tv", &[], dir.path()), Ok(0), "holds nothing");
        let gone = dir.path().join("nowhere");
        assert_eq!(lost_if_forgotten(&lib, "pi", &[], &gone), Ok(2), "no masters folder: both would go");
    }

    #[test]
    fn pins_are_replaced_and_an_empty_choice_leaves_no_record() {
        let mut all = Vec::new();
        assert!(!set_pins(&mut all, "a", &MeshPins::default()), "nothing to keep, nothing written");
        assert!(set_pins(&mut all, "a", &MeshPins { liked: true, playlists: vec![] }));
        assert!(!set_pins(&mut all, "a", &MeshPins { liked: true, playlists: vec![] }), "the same again");
        assert!(set_pins(&mut all, "b", &MeshPins { liked: true, playlists: vec![] }));
        assert_eq!(all.len(), 2);
        assert!(set_pins(&mut all, "a", &MeshPins::default()));
        assert_eq!(all.iter().map(|p| p.node.as_str()).collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn a_pairing_that_could_not_start_says_why() {
        let keep = MeshKeep { failed: Some("Межсеть не запущена".into()), ..MeshKeep::default() };
        assert_eq!(merge(&keep).pairing, MeshPairing::Failed("Межсеть не запущена".into()));
    }

    #[test]
    fn the_sync_switch_and_reports_show_per_device() {
        let live = MeshSnapshot { running: true, paired: true, nodes: vec![node("a", "Pi", true)], ..MeshSnapshot::default() };
        let mut keep = MeshKeep { live: Some(live), ..MeshKeep::default() };
        keep.sync.insert("a".into(), MeshSyncView { running: true, done: 5, total: 10, ..MeshSyncView::default() });
        let v = merge(&keep);
        assert!(v.nodes[0].sync.running && !v.nodes[0].sync.off);
        assert!((v.nodes[0].sync.fraction() - 0.5).abs() < 1e-6);
        keep.sync_off.push("a".into());
        let v = merge(&keep);
        assert!(v.nodes[0].sync.off && !v.nodes[0].sync.running, "off: nothing runs");
        // Without a report from the service, the kept rows show the switch too.
        let keep = MeshKeep { rows: vec![row("b", "Pi")], sync_off: vec!["b".into()], ..MeshKeep::default() };
        assert!(merge(&keep).nodes[0].sync.off);
    }
}
