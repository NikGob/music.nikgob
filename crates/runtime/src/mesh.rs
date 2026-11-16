//! «Межсеть → Устройства» as the loop keeps it: `[mesh]` of settings.toml
//! (the switch, this device's name), the node table (the paired devices,
//! names given here, what each stores), a device being forgotten while its
//! snackbar's «Отменить» is up. The network is the mesh service's: what
//! needs it is passed on ([`MeshKeep::link`]), and what is live comes back
//! as [`Command::MeshUpdate`] and is merged here. Nothing here waits on the
//! network.

use std::sync::Arc;

use nikgob_core::settings::{MeshSettings, Settings};
use nikgob_library::NodeRow;
use nikgob_view::{Command, MeshKind, MeshNodeView, MeshPairing, MeshProfile, MeshScope, MeshSnapshot, MeshView};

use crate::Worker;

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
                v
            })
            .collect(),
        None => keep.rows.iter().filter(|r| !hidden(&r.id)).map(row_view).collect(),
    };
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
    }
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
        self.mesh_publish();
    }

    fn mesh_publish(&mut self) {
        let view = merge(&self.mesh);
        if self.state.read().mesh != view {
            self.publish(move |s| s.mesh = view);
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
    }

    /// Passes a command that needs the network on to the mesh service.
    fn mesh_forward(&mut self, c: Command) {
        match self.mesh.link.clone() {
            Some(f) => {
                self.mesh.failed = None;
                f(c);
            }
            None => {
                if matches!(c, Command::MeshPairNearby { .. } | Command::MeshCreateInvite | Command::MeshJoin { .. }) {
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
            Command::MeshForget { node_id } => {
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
            Command::MeshForgetFailed { node_id, why } => {
                self.mesh.sent.retain(|f| *f != node_id);
                let name = self.mesh.live.as_ref().and_then(|l| l.nodes.iter().find(|n| n.view.id == node_id)).map(|n| {
                    self.mesh.rows.iter().find(|r| r.id == node_id).map_or_else(|| n.view.name.clone(), |r| r.shown_name().to_owned())
                });
                self.mesh_publish();
                self.toast(format!("Не удалось забыть «{}»: {why}", name.unwrap_or_else(|| "устройство".to_owned())), None);
            }
            c @ (Command::MeshPairNearby { .. }
            | Command::MeshCreateInvite
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
        if self.mesh.forgetting.is_empty() {
            return;
        }
        let now = nikgob_library::now_ms();
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

    #[test]
    fn a_pairing_that_could_not_start_says_why() {
        let keep = MeshKeep { failed: Some("Межсеть не запущена".into()), ..MeshKeep::default() };
        assert_eq!(merge(&keep).pairing, MeshPairing::Failed("Межсеть не запущена".into()));
    }
}
