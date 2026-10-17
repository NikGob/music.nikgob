//! Playlists linked to a service (`playlist_remote`, docs/stage3.md §0
//! "Плейлисты", §3.8), by sync mode:
//!
//! * **Follow** (the default): the service's changes come here. The remote
//!   list is compared with the one seen at the last sync (`base_json`, the
//!   merge base): a track the service added is added here (as a stream
//!   track), one it removed is removed here unless the user added it here
//!   again after that sync. Edits made here stay here.
//! * **TwoWay**: the same, and edits made here since the last sync go to the
//!   service too (`playlist_edit`): additions of tracks that have an id in
//!   that service (local-only tracks stay local), removals of tracks the
//!   base had. Switching an unlinked playlist to TwoWay creates the remote
//!   playlist first. Someone else's playlist cannot be edited: it is
//!   followed instead.
//! * **Once**: a snapshot; the link stays (so the mode can be switched
//!   back) but nothing is synced.
//!
//! A track both sides still have is never removed. After every sync the
//! base is the remote list as it now is, with the version (Spotify's
//! snapshot id) it had.
//!
//! When: every [`CHECK_EVERY`] the logged-in services' playlist lists are
//! read (one request; they carry the versions), and a playlist is fetched
//! when its version changed, when the service has no versions and the last
//! sync is older than [`STALE`], or when a TwoWay playlist has edits here.
//! Opening a linked playlist syncs it too (at most every [`OPEN_FRESH`]),
//! and an edit of a TwoWay playlist is pushed [`PUSH_AFTER`] later.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use nikgob_core::model::{EntryId, PlaylistId};
use nikgob_core::streaming::{
    EntityKind, PlaylistSyncMode, RemoteId, Service, StreamError, StreamPlaylist,
};
use nikgob_library::PlaylistRemote;
use nikgob_view::{self as v, LIKED_PLAYLIST};

use super::{Msg, error_text};
use crate::Worker;

/// How often the services' playlist lists are checked.
pub(crate) const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
/// A playlist of a service without versions is fetched after this.
const STALE: Duration = Duration::from_secs(30 * 60);
/// An opened playlist synced this recently is not synced again.
const OPEN_FRESH: Duration = Duration::from_secs(60);
/// An edit here reaches the service of a TwoWay playlist this much later.
const PUSH_AFTER: Duration = Duration::from_secs(10);

pub(crate) enum SyncMsg {
    /// The service's own playlists (with versions), for the links in `due`.
    Listed {
        service: Service,
        result: Result<Vec<StreamPlaylist>, StreamError>,
        due: Vec<Due>,
    },
    Fetched {
        playlist: PlaylistId,
        result: Result<StreamPlaylist, StreamError>,
    },
    Pushed {
        playlist: PlaylistId,
        /// The base if the push went through.
        base: Vec<String>,
        /// The base if it did not (the remote list as fetched).
        remote: Vec<String>,
        version: Option<String>,
        result: Result<Option<String>, StreamError>,
    },
    Created {
        playlist: PlaylistId,
        service: Service,
        result: Result<String, StreamError>,
    },
}

/// A link the periodic check looks at.
pub(crate) struct Due {
    playlist: PlaylistId,
    remote: RemoteId,
    version: Option<String>,
    stale: bool,
    dirty: bool,
}

#[derive(Default)]
pub(crate) struct SyncState {
    last_check: Option<Instant>,
    checking: HashSet<Service>,
    fetching: HashSet<PlaylistId>,
    /// TwoWay playlists edited here, and when to push.
    push_at: HashMap<PlaylistId, Instant>,
    /// The user asked for these: tell how it went.
    announce: HashSet<PlaylistId>,
    errors: HashMap<PlaylistId, String>,
}

/// What a sync changed here and there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) pushed_add: Vec<String>,
    pub(crate) pushed_remove: Vec<String>,
}

/// One entry of the local playlist with the ids its track has in the service.
pub(crate) struct LocalEntry {
    pub(crate) entry: EntryId,
    pub(crate) added_at: i64,
    pub(crate) remotes: Vec<String>,
}

/// What to do, from the three lists (pure, see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Remote ids to add here, in the service's order.
    pub(crate) add_here: Vec<String>,
    pub(crate) remove_here: Vec<EntryId>,
    pub(crate) push_add: Vec<String>,
    pub(crate) push_remove: Vec<String>,
}

/// The three-way merge. `base` is `None` before the first sync; `synced_at`
/// is when it happened; `two_way` allows pushes.
pub(crate) fn plan(
    remote: &[String],
    base: Option<&[String]>,
    local: &[LocalEntry],
    synced_at: i64,
    two_way: bool,
) -> Plan {
    let base: HashSet<&str> = base.unwrap_or_default().iter().map(String::as_str).collect();
    let remote_set: HashSet<&str> = remote.iter().map(String::as_str).collect();
    let here: HashSet<&str> = local.iter().flat_map(|e| e.remotes.iter().map(String::as_str)).collect();
    let mut p = Plan::default();
    let mut adding = HashSet::new();
    for r in remote {
        if !base.contains(r.as_str()) && !here.contains(r.as_str()) && adding.insert(r.as_str()) {
            p.add_here.push(r.clone());
        }
    }
    for e in local {
        let gone = e
            .remotes
            .iter()
            .any(|r| base.contains(r.as_str()) && !remote_set.contains(r.as_str()));
        let kept_there = e.remotes.iter().any(|r| remote_set.contains(r.as_str()));
        // Added here again after the last sync: the user's, it stays.
        if gone && !kept_there && e.added_at <= synced_at {
            p.remove_here.push(e.entry);
        }
    }
    if two_way {
        let mut pushing = HashSet::new();
        for e in local {
            let Some(first) = e.remotes.first() else { continue }; // local only
            let known = e.remotes.iter().any(|r| base.contains(r.as_str()) || remote_set.contains(r.as_str()));
            if !known && pushing.insert(first.as_str()) {
                p.push_add.push(first.clone());
            }
        }
        for r in remote {
            if base.contains(r.as_str()) && !here.contains(r.as_str()) && !p.push_remove.contains(r) {
                p.push_remove.push(r.clone());
            }
        }
    }
    p
}

fn parse_base(json: Option<&str>) -> Option<Vec<String>> {
    serde_json::from_str(json?).ok()
}

fn ago(ms: i64) -> String {
    let mins = (nikgob_library::now_ms() - ms).max(0) / 60_000;
    match mins {
        0 => "только что".into(),
        1..=59 => format!("{mins} мин назад"),
        60..=1439 => format!("{} ч назад", mins / 60),
        _ => format!("{} дн. назад", mins / 1440),
    }
}

impl Worker {
    /// The entries of a playlist with the ids of their tracks in `service`.
    fn local_entries(&self, playlist: PlaylistId, service: Service) -> Vec<LocalEntry> {
        self.lib
            .playlist_entries(playlist, 0, u32::MAX)
            .unwrap_or_default()
            .into_iter()
            .map(|e| LocalEntry {
                entry: e.entry.id,
                added_at: e.entry.added_at,
                remotes: self
                    .lib
                    .external_ids(EntityKind::Track, *e.track.id.as_uuid())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.service == service)
                    .map(|r| r.id)
                    .collect(),
            })
            .collect()
    }

    /// The time a sync of `playlist` is recorded at: now, but never before
    /// an entry it just added (entry stamps run a little ahead under load),
    /// so nothing the sync itself added counts as added here afterwards.
    pub(crate) fn sync_stamp(&self, playlist: PlaylistId) -> i64 {
        self.lib
            .playlist_entries(playlist, 0, u32::MAX)
            .unwrap_or_default()
            .iter()
            .map(|e| e.entry.added_at)
            .fold(nikgob_library::now_ms(), i64::max)
    }

    /// A TwoWay playlist has edits here the service has not seen.
    fn dirty(&self, link: &PlaylistRemote) -> bool {
        if link.sync_mode != PlaylistSyncMode::TwoWay {
            return false;
        }
        let Some(base) = parse_base(link.base_json.as_deref()) else {
            return true;
        };
        let base: HashSet<String> = base.into_iter().collect();
        let local = self.local_entries(link.playlist_id, link.remote.service);
        let here: HashSet<&String> = local.iter().flat_map(|e| &e.remotes).collect();
        base.iter().any(|b| !here.contains(b))
            || local.iter().any(|e| !e.remotes.is_empty() && !e.remotes.iter().any(|r| base.contains(r)))
    }

    /// The live links that sync (not Once, playlist not deleted).
    fn live_links(&self) -> Vec<PlaylistRemote> {
        self.lib
            .playlist_remotes()
            .unwrap_or_default()
            .into_iter()
            .filter(|l| l.sync_mode != PlaylistSyncMode::Once)
            .filter(|l| self.lib.get_playlist(l.playlist_id).ok().flatten().is_some())
            .collect()
    }

    /// From the 5 s tick: due pushes, and the periodic check.
    pub(crate) fn sync_tick(&mut self) {
        let now = Instant::now();
        let due: Vec<PlaylistId> = self
            .streaming
            .sync
            .push_at
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(p, _)| *p)
            .collect();
        for p in due {
            self.streaming.sync.push_at.remove(&p);
            self.sync_playlist(p);
        }
        if self.streaming.sync.last_check.is_some_and(|t| t.elapsed() < CHECK_EVERY) {
            return;
        }
        self.streaming.sync.last_check = Some(now);
        let links = self.live_links();
        let now_ms = nikgob_library::now_ms();
        for &service in Service::ALL {
            let Some(svc) = self.service(service) else { continue };
            if self.streaming.sync.checking.contains(&service) {
                continue;
            }
            let due: Vec<Due> = links
                .iter()
                .filter(|l| l.remote.service == service)
                .map(|l| Due {
                    playlist: l.playlist_id,
                    remote: l.remote.clone(),
                    version: l.remote_version.clone(),
                    stale: l.synced_at.is_none_or(|t| now_ms - t >= STALE.as_millis() as i64),
                    dirty: self.dirty(l),
                })
                .collect();
            if due.is_empty() {
                continue;
            }
            if !svc.account().is_logged_in() {
                // No list without a login: fetch what is stale.
                for d in due.into_iter().filter(|d| d.stale) {
                    self.sync_playlist(d.playlist);
                }
                continue;
            }
            self.streaming.sync.checking.insert(service);
            self.streaming.spawn(async move {
                let result = svc.my_playlists().await;
                Msg::Sync(SyncMsg::Listed { service, result, due })
            });
        }
    }

    /// Fetches a linked playlist and merges it (see the module docs).
    pub(crate) fn sync_playlist(&mut self, playlist: PlaylistId) {
        let Some(link) = self.lib.playlist_remote(playlist).ok().flatten() else { return };
        if link.sync_mode == PlaylistSyncMode::Once {
            return;
        }
        let Some(svc) = self.service(link.remote.service) else { return };
        if !self.streaming.sync.fetching.insert(playlist) {
            return;
        }
        self.publish_playlist_sync(playlist);
        let id = link.remote.id.clone();
        self.streaming.spawn(async move {
            let result = svc.playlist(&id).await;
            Msg::Sync(SyncMsg::Fetched { playlist, result })
        });
    }

    /// `Command::LoadPlaylist`: an opened linked playlist is synced.
    pub(crate) fn sync_on_open(&mut self, playlist: PlaylistId) {
        let Some(link) = self.lib.playlist_remote(playlist).ok().flatten() else { return };
        let fresh = link
            .synced_at
            .is_some_and(|t| nikgob_library::now_ms() - t < OPEN_FRESH.as_millis() as i64);
        if !fresh {
            self.sync_playlist(playlist);
        }
    }

    /// A playlist was edited here: a TwoWay one is pushed soon.
    pub(crate) fn playlist_edited(&mut self, playlist: PlaylistId) {
        if let Some(link) = self.lib.playlist_remote(playlist).ok().flatten()
            && link.sync_mode == PlaylistSyncMode::TwoWay
        {
            self.streaming.sync.push_at.insert(playlist, Instant::now() + PUSH_AFTER);
        }
    }

    pub(crate) fn sync_msg(&mut self, m: SyncMsg) {
        match m {
            SyncMsg::Listed { service, result, due } => {
                self.streaming.sync.checking.remove(&service);
                let list = result.unwrap_or_else(|e| {
                    log::warn!("{service}: my_playlists: {e}");
                    Vec::new()
                });
                for d in due {
                    let listed = list.iter().find(|p| p.id == d.remote);
                    let changed = match listed.map(|p| &p.version) {
                        Some(Some(v)) => d.version.as_deref() != Some(v.as_str()),
                        // No version in the service, or not in the list
                        // (someone else's playlist): by age.
                        _ => d.stale,
                    };
                    if changed || d.dirty {
                        self.sync_playlist(d.playlist);
                    }
                }
            }
            SyncMsg::Fetched { playlist, result } => {
                self.streaming.sync.fetching.remove(&playlist);
                match result {
                    Ok(p) => {
                        self.streaming.learn(&p.tracks);
                        self.streaming.playlists.insert(p.id.clone(), p.clone());
                        self.apply_sync(playlist, &p);
                    }
                    Err(e) => {
                        log::warn!("sync {playlist}: {e}");
                        self.streaming.sync.errors.insert(playlist, error_text(&e));
                        if self.streaming.sync.announce.remove(&playlist) {
                            self.toast(format!("Синхронизация не удалась: {}", error_text(&e)), None);
                        }
                        self.publish_playlist_sync(playlist);
                    }
                }
            }
            SyncMsg::Pushed {
                playlist,
                base,
                remote,
                version,
                result,
            } => {
                let now = self.sync_stamp(playlist);
                match result {
                    Ok(new_version) => {
                        let json = serde_json::to_string(&base).ok();
                        let _ = self.lib.mark_playlist_synced(playlist, new_version.or(version).as_deref(), now, json.as_deref());
                        self.streaming.sync.errors.remove(&playlist);
                    }
                    Err(e) => {
                        log::warn!("sync {playlist}: push: {e}");
                        // The base stays the remote list: the edits here
                        // differ from it, so they are pushed next time.
                        let json = serde_json::to_string(&remote).ok();
                        let _ = self.lib.mark_playlist_synced(playlist, version.as_deref(), now, json.as_deref());
                        self.streaming.sync.errors.insert(playlist, error_text(&e));
                        self.toast(format!("Не получилось обновить плейлист в сервисе: {}", error_text(&e)), None);
                    }
                }
                self.publish_playlist_sync(playlist);
            }
            SyncMsg::Created { playlist, service, result } => match result {
                Ok(id) => {
                    let _ = self.lib.put_playlist_remote(&PlaylistRemote {
                        playlist_id: playlist,
                        remote: RemoteId::new(service, id),
                        sync_mode: PlaylistSyncMode::TwoWay,
                        remote_version: None,
                        synced_at: None,
                        base_json: None,
                    });
                    self.sync_playlist(playlist);
                }
                Err(e) => {
                    self.streaming.sync.announce.remove(&playlist);
                    self.toast(
                        format!("Не получилось создать плейлист в {}: {}", service.display_name(), error_text(&e)),
                        None,
                    );
                }
            },
        }
    }

    /// Merges the fetched remote playlist into the linked library one;
    /// pushes the edits here for TwoWay.
    pub(crate) fn apply_sync(&mut self, playlist: PlaylistId, remote: &StreamPlaylist) -> Outcome {
        let Some(mut link) = self.lib.playlist_remote(playlist).ok().flatten() else {
            return Outcome::default();
        };
        if link.sync_mode == PlaylistSyncMode::Once
            || self.lib.get_playlist(playlist).ok().flatten().is_none()
        {
            return Outcome::default();
        }
        if link.sync_mode == PlaylistSyncMode::TwoWay && !remote.editable {
            link.sync_mode = PlaylistSyncMode::Follow;
            let _ = self.lib.put_playlist_remote(&link);
            self.toast(
                format!("«{}» в {} менять нельзя — только слежка", remote.name, remote.id.service.display_name()),
                None,
            );
        }
        let service = link.remote.service;
        let remote_ids: Vec<String> = remote.tracks.iter().map(|t| t.id.id.clone()).collect();
        let base = parse_base(link.base_json.as_deref());
        let local = self.local_entries(playlist, service);
        let two_way = link.sync_mode == PlaylistSyncMode::TwoWay;
        let p = plan(&remote_ids, base.as_deref(), &local, link.synced_at.unwrap_or(0), two_way);
        let mut out = Outcome::default();
        if !p.add_here.is_empty() {
            let tracks: Vec<_> = p
                .add_here
                .iter()
                .filter_map(|id| remote.tracks.iter().find(|t| t.id.id == *id).cloned())
                .collect();
            let ids = self.upsert_streams(&tracks);
            out.added = ids.len();
            let _ = self.log_err("плейлист", self.lib.playlist_add_many(playlist, &ids));
        }
        for e in &p.remove_here {
            if self.lib.playlist_remove(*e).is_ok() {
                out.removed += 1;
            }
        }
        let now = self.sync_stamp(playlist);
        let pushes = two_way && !(p.push_add.is_empty() && p.push_remove.is_empty());
        let svc = self.service(service).filter(|s| s.account().is_logged_in());
        match svc.filter(|_| pushes) {
            Some(svc) => {
                let mut base: Vec<String> = remote_ids
                    .iter()
                    .filter(|r| !p.push_remove.contains(r))
                    .cloned()
                    .collect();
                base.extend(p.push_add.iter().cloned());
                out.pushed_add.clone_from(&p.push_add);
                out.pushed_remove.clone_from(&p.push_remove);
                let (id, add, remove) = (link.remote.id.clone(), p.push_add, p.push_remove);
                let version = remote.version.clone();
                let remote_list = remote_ids.clone();
                self.streaming.spawn(async move {
                    let result = svc.playlist_edit(&id, &add, &remove).await;
                    Msg::Sync(SyncMsg::Pushed {
                        playlist,
                        base,
                        remote: remote_list,
                        version,
                        result,
                    })
                });
            }
            None => {
                let json = serde_json::to_string(&remote_ids).ok();
                let _ = self.lib.mark_playlist_synced(playlist, remote.version.as_deref(), now, json.as_deref());
                self.streaming.sync.errors.remove(&playlist);
            }
        }
        log::info!(
            "sync {playlist} ({:?}): +{} −{} here, +{} −{} there",
            link.sync_mode,
            out.added,
            out.removed,
            out.pushed_add.len(),
            out.pushed_remove.len()
        );
        if self.streaming.sync.announce.remove(&playlist) {
            let there = out.pushed_add.len() + out.pushed_remove.len();
            let text = if out.added + out.removed + there == 0 {
                "Плейлист синхронизирован: изменений нет".to_owned()
            } else {
                format!(
                    "Плейлист синхронизирован: +{} −{} здесь, {} в {}",
                    out.added,
                    out.removed,
                    there,
                    service.display_name()
                )
            };
            self.toast(text, None);
        }
        if out.added + out.removed > 0 {
            self.load_library();
        }
        self.publish_playlist_sync(playlist);
        out
    }

    /// `Command::SetPlaylistSync`.
    pub(crate) fn set_playlist_sync(&mut self, playlist: PlaylistId, mode: PlaylistSyncMode) {
        if playlist == LIKED_PLAYLIST {
            self.toast("«Любимые» синхронизируются лайками", None);
            return;
        }
        let Some(local) = self.lib.get_playlist(playlist).ok().flatten() else { return };
        let label = match mode {
            PlaylistSyncMode::TwoWay => "двусторонняя",
            PlaylistSyncMode::Follow => "только слежка",
            PlaylistSyncMode::Once => "разово (снимок)",
        };
        match self.lib.playlist_remote(playlist).ok().flatten() {
            Some(mut link) => {
                link.sync_mode = mode;
                let _ = self.log_err("плейлист", self.lib.put_playlist_remote(&link));
                self.toast(format!("Синхронизация: {label}"), None);
                if mode != PlaylistSyncMode::Once {
                    self.streaming.sync.announce.insert(playlist);
                    self.sync_playlist(playlist);
                }
            }
            None if mode == PlaylistSyncMode::TwoWay => {
                let Some(service) = self.service_for_new_remote(playlist) else {
                    self.toast("Войди в Spotify или YouTube Music, чтобы связать плейлист", None);
                    return;
                };
                let Some(svc) = self.service(service) else { return };
                self.toast(format!("Создаю «{}» в {}…", local.name, service.display_name()), None);
                self.streaming.sync.announce.insert(playlist);
                let name = local.name.clone();
                self.streaming.spawn(async move {
                    let result = svc.create_playlist(&name).await;
                    Msg::Sync(SyncMsg::Created { playlist, service, result })
                });
            }
            None => self.toast("Плейлист не связан с сервисом", None),
        }
        self.publish_playlist_sync(playlist);
    }

    /// Where a local playlist goes when it becomes TwoWay: the logged-in
    /// service most of its tracks have ids in (Spotify on a tie).
    fn service_for_new_remote(&self, playlist: PlaylistId) -> Option<Service> {
        let entries = self.lib.playlist_entries(playlist, 0, u32::MAX).unwrap_or_default();
        Service::ALL
            .iter()
            .copied()
            .filter(|s| self.service(*s).is_some_and(|svc| svc.account().is_logged_in()))
            .map(|s| {
                let n = entries
                    .iter()
                    .filter(|e| {
                        self.lib
                            .external_ids(EntityKind::Track, *e.track.id.as_uuid())
                            .unwrap_or_default()
                            .iter()
                            .any(|r| r.service == s)
                    })
                    .count();
                (s, n)
            })
            // max_by_key keeps the last of equals: reverse so Spotify wins.
            .rev()
            .max_by_key(|(_, n)| *n)
            .map(|(s, _)| s)
    }

    /// The sync line of a playlist page ([`v::PlaylistDetail::sync`]).
    pub(crate) fn playlist_sync_view(&self, playlist: PlaylistId) -> Option<v::PlaylistSyncView> {
        let link = self.lib.playlist_remote(playlist).ok().flatten()?;
        Some(v::PlaylistSyncView {
            remote: link.remote.clone(),
            mode: link.sync_mode,
            synced: link.synced_at.map(ago),
            syncing: self.streaming.sync.fetching.contains(&playlist),
            error: self.streaming.sync.errors.get(&playlist).cloned(),
        })
    }

    fn publish_playlist_sync(&mut self, playlist: PlaylistId) {
        let open = self.state.read().playlist.as_ref().is_some_and(|p| p.card.id == playlist);
        if open {
            self.load_playlist(playlist);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn entry(n: u128, remote: &[&str], added_at: i64) -> LocalEntry {
        LocalEntry {
            entry: EntryId::from_uuid(nikgob_core::model::Uuid::from_u128(n)),
            added_at,
            remotes: ids(remote),
        }
    }

    #[test]
    fn follow_takes_remote_edits_and_keeps_local_ones() {
        let base = ids(&["a", "b", "c"]);
        let remote = ids(&["a", "c", "d"]); // b removed, d added there
        let local = [entry(1, &["a"], 0), entry(2, &["b"], 0), entry(3, &["c"], 0), entry(4, &[], 50)];
        let p = plan(&remote, Some(&base), &local, 100, false);
        assert_eq!(p.add_here, ids(&["d"]));
        assert_eq!(p.remove_here, vec![local[1].entry]);
        assert!(p.push_add.is_empty() && p.push_remove.is_empty());

        // Removed here (c), added here (x): Follow leaves both as they are.
        let local = [entry(1, &["a"], 0), entry(5, &["x"], 150)];
        let p = plan(&ids(&["a", "c"]), Some(&ids(&["a", "c"])), &local, 100, false);
        assert_eq!(p, Plan::default());
    }

    #[test]
    fn a_track_added_here_again_after_the_sync_stays() {
        let local = [entry(1, &["b"], 200)];
        let p = plan(&ids(&[]), Some(&ids(&["b"])), &local, 100, false);
        assert!(p.remove_here.is_empty());
    }

    #[test]
    fn two_way_pushes_edits_with_ids_only() {
        let base = ids(&["a", "b"]);
        let remote = ids(&["a", "b", "r"]);
        // b removed here, x and local-only added here.
        let local = [entry(1, &["a"], 0), entry(2, &["x"], 150), entry(3, &[], 150)];
        let p = plan(&remote, Some(&base), &local, 100, true);
        assert_eq!(p.add_here, ids(&["r"]));
        assert_eq!(p.push_add, ids(&["x"]));
        assert_eq!(p.push_remove, ids(&["b"]));
        assert!(p.remove_here.is_empty());
    }

    #[test]
    fn first_sync_merges_without_removing() {
        let local = [entry(1, &["a"], 0), entry(2, &["x"], 0)];
        let p = plan(&ids(&["a", "b"]), None, &local, 0, true);
        assert_eq!(p.add_here, ids(&["b"]));
        assert_eq!(p.push_add, ids(&["x"]));
        assert!(p.remove_here.is_empty() && p.push_remove.is_empty());
    }

    #[test]
    fn both_sides_removed_or_both_added_is_no_conflict() {
        let local = [entry(1, &["a"], 0), entry(2, &["n"], 150)];
        // c removed on both sides; n added on both sides.
        let p = plan(&ids(&["a", "n"]), Some(&ids(&["a", "c"])), &local, 100, true);
        assert_eq!(p, Plan::default());
    }
}
