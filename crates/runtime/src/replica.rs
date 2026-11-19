//! «Оригиналы FLAC на ПК» (`[mesh] keep_masters`): while it is not
//! «Всегда», the lossless masters this computer still has whose verified
//! copies are on enough of the owner's trusted devices are offered on a
//! «Подтверди» card, each file that would move listed; nothing goes by
//! itself. On «Убрать в Корзину» the devices are asked to read their copies
//! again right then (the mesh, [`Command::ReplicaRecheck`]) and only the
//! files of the masters that still have enough go to the Trash, each looked
//! at again right before it moves; a change of the trusted devices or of
//! the choice meanwhile makes the answer void. «Отменить» brings them back,
//! the Trash's 30 days end them. The master rows stay: the files exist on
//! the devices.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use nikgob_core::i18n::activity::fmt_size_in;
use nikgob_core::i18n::{Locale, activity_in, plural_forms};
use nikgob_library::Library;
use nikgob_view::{Command, Confirm, ConfirmOption};

use crate::{Later, Worker};

/// Bit of the «Подтверди» ids of the masters that may go.
pub(crate) const REPLICA_ID: u64 = 1 << 43;
/// The masters that may go are looked at this often (and at once when the
/// devices' copies or the choice change).
const PLAN_EVERY: Duration = Duration::from_secs(10 * 60);
/// Masters one card lists at most (the next ones come after it).
const CARD_MAX: usize = 50;
/// The devices' second look at their copies is waited for this long.
const RECHECK_WAIT: Duration = Duration::from_secs(20 * 60);

/// A master file on the card: one line for each file that would move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Offered {
    pub(crate) hash: String,
    /// The file itself.
    pub(crate) path: PathBuf,
    /// The song: «Title — Artist».
    pub(crate) label: String,
    /// Where its copies are, its size and where the file is.
    pub(crate) detail: String,
}

/// What the masters that may go depend on: the choice (how many copies)
/// and the trusted devices staying. A change makes an answer void.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Watch {
    need: Option<u32>,
    devices: Vec<String>,
}

/// What the thread moving masters looks at right before each file goes,
/// under the lock the worker takes to change it (so a forget or another
/// choice lands before or after a removal, never between the look and
/// it): the count of changes, the choice and the devices being forgotten
/// as they are now.
#[derive(Debug, Default)]
pub(crate) struct MoveWatch {
    era: u64,
    need: Option<u32>,
    leaving: Vec<String>,
}

/// The last look before one master's file goes ([`MoveWatch`]): nothing
/// changed since the owner said yes, and the trusted devices staying hold
/// verified copies of it as many as the choice wants now.
struct Gate {
    watch: Arc<Mutex<MoveWatch>>,
    era: u64,
    lib: Library,
    hash: String,
}

impl nikgob_import::trash::MoveGate for Gate {
    fn through(&self, step: &mut dyn FnMut() -> std::io::Result<()>) -> Option<std::io::Result<()>> {
        let w = self.watch.lock();
        let need = w.need.filter(|_| w.era == self.era)?;
        let holders = self.lib.verified_holders(&self.hash, &w.leaving).ok()?;
        (holders.len() >= need as usize).then(|| step())
    }
}

/// What an accepted card is doing.
enum Busy {
    /// Its masters' copies are being read again on the devices, with the
    /// state of things it was asked in.
    Rechecking { card: u64, items: Vec<Offered>, since: Instant, era: u64 },
    /// The ones that may go are being moved to the Trash.
    Moving,
}

/// What the loop keeps of the masters that may go.
#[derive(Default)]
pub(crate) struct ReplicaKeep {
    /// A look at them runs on its thread.
    planning: bool,
    /// When the next look is due (None: at the next tick).
    due: Option<Instant>,
    /// The card shown: its id and what it lists.
    card: Option<(u64, Vec<Offered>)>,
    next_card: u64,
    /// The masters the owner chose to keep (or took back), not offered
    /// again while the player runs.
    kept: HashSet<String>,
    busy: Option<Busy>,
    /// What the masters that may go depend on, as last seen, and what the
    /// thread moving them looks at before each file.
    watch: Option<Watch>,
    gate: Arc<Mutex<MoveWatch>>,
}

/// «Устройства не ответили — оригиналы остались на компьютере».
fn stay_text(loc: Locale, reason: &str) -> String {
    format!("{reason} — {}", activity_in(loc).masters_stay)
}

/// The card of master files that may go (`id`): each file listed; yes
/// moves exactly those to the Trash, no keeps them.
pub(crate) fn keep_card(id: u64, items: &[Offered], loc: Locale) -> Confirm {
    let s = activity_in(loc);
    let n = items.len();
    Confirm {
        id,
        title: s.masters_title.to_owned(),
        origin: format!("{} · {n} {}", s.masters_origin, plural_forms(loc, n as u64, &s.files)),
        question: s.masters_question.to_owned(),
        options: items.iter().map(|o| ConfirmOption { label: o.label.clone(), detail: o.detail.clone(), recommended: false }).collect(),
        skip_label: s.masters_skip.to_owned(),
        accept_label: s.masters_accept.to_owned(),
        pick: None,
        yes_no: true,
    }
}

/// The master files to offer: every file this computer still has (a plain
/// file inside `masters_dir`: kept by name, or flat) of the masters `need`
/// trusted devices (none of `leaving`) hold verified copies of, not
/// `kept`; one line each, with where it is. Reads the library and the
/// masters folder: never on the loop.
pub(crate) fn offer_list(lib: &Library, need: u32, leaving: &[String], masters_dir: &Path, kept: &HashSet<String>, loc: Locale) -> Result<Vec<Offered>, String> {
    let all = lib.masters_to_let_go(need, leaving).map_err(|e| e.to_string())?;
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let names: HashMap<String, String> =
        lib.nodes().map(|rows| rows.into_iter().map(|r| (r.id.clone(), r.shown_name().to_owned())).collect()).unwrap_or_default();
    let mut out = Vec::new();
    for g in all {
        if kept.contains(&g.file_hash) {
            continue;
        }
        let mut files: Vec<PathBuf> = g.paths.iter().map(PathBuf::from).collect();
        if g.flat {
            files.extend(nikgob_import::master_files::flat_master(masters_dir, &g.file_hash));
        }
        files.retain(|p| nikgob_import::fsutil::plain_file_inside(p, masters_dir));
        files.dedup();
        if files.is_empty() {
            continue;
        }
        let label = g
            .tracks
            .first()
            .and_then(|t| lib.get_track_row(*t).ok().flatten())
            .map_or_else(|| activity_in(loc).untitled.to_owned(), |r| if r.artist.trim().is_empty() { r.title } else { format!("{} — {}", r.title, r.artist) });
        let on: Vec<String> = g.holders.iter().map(|h| names.get(h).cloned().unwrap_or_else(|| h.chars().take(8).collect())).collect();
        for path in files {
            let shown = path.strip_prefix(masters_dir).unwrap_or(&path).display().to_string();
            out.push(Offered {
                hash: g.file_hash.clone(),
                label: label.clone(),
                detail: format!("{} · {} · {shown}", on.join(", "), fmt_size_in(loc, g.size)),
                path,
            });
        }
    }
    Ok(out)
}

impl Worker {
    /// The masters that may go are looked at again at the next tick (the
    /// choice or the devices' copies changed).
    pub(crate) fn replica_soon(&mut self) {
        self.replica.due = None;
    }

    /// The choice and the trusted devices staying, as they are now.
    fn replica_watch_now(&self) -> Watch {
        let leaving = self.mesh.leaving();
        let st = self.state.read();
        let mut devices: Vec<String> = st
            .mesh
            .nodes
            .iter()
            .filter(|n| n.scope == nikgob_view::MeshScope::Owner && !leaving.contains(&n.id))
            .map(|n| n.id.clone())
            .collect();
        devices.sort();
        Watch { need: st.mesh.keep_masters.copies(), devices }
    }

    /// A change of the choice or of the trusted devices (called at once
    /// when the mesh's state is published, a forget among them): counted
    /// under the movers' lock before it returns (the thread moving masters
    /// stops before its next file), and a second look under way is void.
    pub(crate) fn replica_follow(&mut self) {
        let now = self.replica_watch_now();
        if self.replica.watch.as_ref() == Some(&now) {
            return;
        }
        let first = self.replica.watch.is_none();
        {
            let leaving = self.mesh.leaving();
            let mut g = self.replica.gate.lock();
            g.need = now.need;
            g.leaving = leaving;
            if !first {
                g.era += 1;
            }
        }
        self.replica.watch = Some(now);
        if first {
            return;
        }
        self.replica.due = None;
        if matches!(self.replica.busy, Some(Busy::Rechecking { .. })) {
            self.replica.busy = None;
            self.toast_info(stay_text(self.locale(), activity_in(self.locale()).masters_devices_changed), None);
        }
    }

    /// The loop's tick: a look at the masters that may go when due (only
    /// while the choice lets any go); a second look the devices never
    /// answered given up.
    pub(crate) fn replica_tick(&mut self) {
        self.replica_follow();
        if let Some(Busy::Rechecking { since, .. }) = &self.replica.busy
            && since.elapsed() > RECHECK_WAIT
        {
            self.replica.busy = None;
            self.toast_err(stay_text(self.locale(), activity_in(self.locale()).masters_no_answer), None);
        }
        let Some(need) = self.state.read().mesh.keep_masters.copies() else {
            if self.replica.card.take().is_some() {
                self.replica_publish();
            }
            return;
        };
        if self.replica.planning || self.replica.busy.is_some() || self.replica.due.is_some_and(|d| Instant::now() < d) {
            return;
        }
        let Some(svc) = &self.import else { return };
        let masters_dir = svc.config().masters_dir();
        let (lib, later) = (self.lib.clone(), self.later_tx.clone());
        let (leaving, kept) = (self.mesh.leaving(), self.replica.kept.clone());
        let loc = self.locale();
        self.replica.planning = true;
        let spawned = std::thread::Builder::new().name("nikgob-masters-plan".into()).spawn(move || {
            crate::quiet::lower_priority();
            let out = offer_list(&lib, need, &leaving, &masters_dir, &kept, loc);
            let _ = later.send(Later::new("the masters that may go", move |w| w.replica_planned(out)));
        });
        if let Err(e) = spawned {
            log::warn!("replica: the masters that may go not looked at: {e}");
            self.replica.planning = false;
            self.replica.due = Some(Instant::now() + PLAN_EVERY);
        }
    }

    fn replica_planned(&mut self, out: Result<Vec<Offered>, String>) {
        self.replica.planning = false;
        self.replica.due = Some(Instant::now() + PLAN_EVERY);
        if self.replica.busy.is_some() || self.state.read().mesh.keep_masters.copies().is_none() {
            return;
        }
        let items = match out {
            Ok(items) => items,
            Err(e) => {
                log::warn!("replica: the masters that may go not read: {e}");
                return;
            }
        };
        // Whole masters only: every file of a master on the card, or none.
        let mut items: Vec<Offered> = items.into_iter().filter(|o| !self.replica.kept.contains(&o.hash)).collect();
        let mut end = 0;
        while end < items.len() {
            let h = items[end].hash.clone();
            let next = end + items[end..].iter().take_while(|o| o.hash == h).count();
            if next > CARD_MAX && end > 0 {
                break;
            }
            end = next;
        }
        items.truncate(end);
        let card = if items.is_empty() {
            None
        } else {
            let id = match &self.replica.card {
                Some((id, old)) if *old == items => *id,
                _ => {
                    self.replica.next_card += 1;
                    REPLICA_ID | self.replica.next_card
                }
            };
            Some((id, items))
        };
        if card != self.replica.card {
            self.replica.card = card;
            self.replica_publish();
        }
    }

    fn replica_publish(&mut self) {
        let card = self.replica.card.as_ref().map(|(id, items)| keep_card(*id, items));
        self.publish(move |s| {
            s.downloads.confirm.retain(|c| c.id & REPLICA_ID == 0);
            s.downloads.confirm.extend(card);
        });
    }

    /// The card's answer: «Оставить» keeps its masters (not offered again
    /// while the player runs); «Убрать в Корзину» asks the devices to read
    /// their copies again first.
    pub(crate) fn replica_choice(&mut self, id: u64, option: Option<usize>) {
        let Some((card, items)) = self.replica.card.clone().filter(|(c, _)| *c == id) else {
            log::info!("replica: an answer to a card no longer shown");
            return;
        };
        let mut hashes: Vec<String> = items.iter().map(|o| o.hash.clone()).collect();
        hashes.dedup();
        let need = self.state.read().mesh.keep_masters.copies();
        let (Some(_), Some(need)) = (option, need) else {
            self.replica.kept.extend(hashes);
            self.replica.card = None;
            self.replica.due = None;
            self.replica_publish();
            return;
        };
        let Some(link) = self.mesh.link.clone() else {
            self.replica.card = None;
            self.replica.due = None;
            self.replica_publish();
            self.toast_err(stay_text(self.locale(), activity_in(self.locale()).mesh_off), None);
            return;
        };
        self.replica_follow();
        let era = self.replica.gate.lock().era;
        self.replica.card = None;
        self.replica.busy = Some(Busy::Rechecking { card, items, since: Instant::now(), era });
        self.replica_publish();
        let leaving = self.mesh.leaving();
        log::info!("replica: {} masters may go; their copies are read again first", hashes.len());
        link(Command::ReplicaRecheck { card, hashes, need, leaving });
    }

    /// The devices' second look: the files of the masters that still have
    /// enough verified copies go to the Trash (exactly those of the card),
    /// each looked at again right before it moves; the others stay. Void
    /// when the trusted devices or the choice changed since it was asked.
    pub(crate) fn replica_rechecked(&mut self, card: u64, ok: Vec<String>, failed: Option<String>) {
        self.replica_follow();
        let (items, era) = match &self.replica.busy {
            Some(Busy::Rechecking { card: c, items, era, .. }) if *c == card => (items.clone(), *era),
            _ => return,
        };
        let Some(need) = self.state.read().mesh.keep_masters.copies() else {
            self.replica.busy = None;
            self.toast_info(stay_text(self.locale(), activity_in(self.locale()).masters_choice_changed), None);
            return;
        };
        if self.replica.gate.lock().era != era {
            self.replica.busy = None;
            self.toast_info(stay_text(self.locale(), activity_in(self.locale()).masters_devices_changed), None);
            return;
        }
        let asked: HashSet<String> = items.iter().map(|o| o.hash.clone()).collect();
        let mut ok: Vec<String> = ok.into_iter().filter(|h| asked.contains(h)).collect();
        ok.sort();
        ok.dedup();
        let stayed = asked.len() - ok.len();
        if ok.is_empty() {
            self.replica.busy = None;
            self.replica.due = None;
            if let Some(why) = failed {
                log::warn!("replica: the copies were not read again: {why}");
                self.toast_err(stay_text(self.locale(), activity_in(self.locale()).masters_unverified), None);
            } else {
                self.toast_info(stay_text(self.locale(), activity_in(self.locale()).masters_too_few), None);
            }
            return;
        }
        let Some(svc) = &self.import else {
            self.replica.busy = None;
            self.toast_err(stay_text(self.locale(), activity_in(self.locale()).masters_import_unavailable), None);
            return;
        };
        let (trash, masters) = (svc.config().trash_dir.clone(), svc.config().masters_dir());
        let (lib, later) = (self.lib.clone(), self.later_tx.clone());
        let watch = self.replica.gate.clone();
        log::debug!("replica: {need} copies wanted when the masters go");
        self.replica.busy = Some(Busy::Moving);
        let spawned = std::thread::Builder::new().name("nikgob-masters-trash".into()).spawn(move || {
            let (mut ids, mut left) = (Vec::new(), stayed);
            for h in &ok {
                let files: Vec<PathBuf> = items.iter().filter(|o| &o.hash == h).map(|o| o.path.clone()).collect();
                // Right before each file, under the lock a forget or another
                // choice takes: nothing changed since, and the trusted devices
                // staying still hold enough verified copies.
                let gate = Gate { watch: watch.clone(), era, lib: lib.clone(), hash: h.clone() };
                let moved = nikgob_import::trash::trash_master_files(&lib, &trash, &masters, h, &files, &gate);
                if let Some(e) = &moved.error {
                    log::warn!("replica: the master {h} stays (in part): {e}");
                }
                if moved.ids.is_empty() || moved.error.is_some() {
                    left += 1;
                }
                // Every file moved can come back, also when a later one
                // failed.
                ids.extend(moved.ids);
            }
            let _ = later.send(Later::new("masters moved to the Trash", move |w| w.replica_trashed(ids, left)));
        });
        if let Err(e) = spawned {
            log::warn!("replica: the masters not moved: {e}");
            self.replica.busy = None;
        }
    }

    fn replica_trashed(&mut self, ids: Vec<i64>, stayed: usize) {
        self.replica.busy = None;
        self.replica.due = None;
        let loc = self.locale();
        let s = activity_in(loc);
        if ids.is_empty() {
            self.toast_info(s.masters_stayed, None);
            return;
        }
        let n = ids.len();
        log::info!("replica: {n} master files moved to the Trash, {stayed} masters stay");
        let text = if stayed > 0 {
            format!("{}: {n}, {}: {stayed}", s.masters_trashed, s.masters_stayed_count)
        } else {
            format!("{}: {n}", s.masters_trashed)
        };
        let undo = self.undo_action(Command::ReplicaUndoTrash { ids });
        self.toast_ok(text, Some(undo));
    }

    /// «Отменить»: the master files go back where they were (never over a
    /// file that took a place) and are not offered again while the player
    /// runs.
    pub(crate) fn replica_undo(&mut self, ids: Vec<i64>) {
        let (lib, later) = (self.lib.clone(), self.later_tx.clone());
        let spawned = std::thread::Builder::new().name("nikgob-masters-back".into()).spawn(move || {
            let (mut back, mut failed) = (Vec::new(), 0usize);
            for id in ids {
                let hash = lib.master_trash_row(id).ok().flatten().map(|r| r.file_hash);
                match nikgob_import::trash::restore_trashed_master(&lib, id) {
                    Ok(true) => back.extend(hash),
                    Ok(false) => failed += 1,
                    Err(e) => {
                        log::warn!("replica: a master stays in the Trash: {e}");
                        failed += 1;
                    }
                }
            }
            let _ = later.send(Later::new("masters back from the Trash", move |w| w.replica_undone(back, failed)));
        });
        if let Err(e) = spawned {
            log::warn!("replica: the masters not brought back: {e}");
        }
    }

    fn replica_undone(&mut self, back: Vec<String>, failed: usize) {
        let n = back.len();
        self.replica.kept.extend(back);
        self.replica.due = None;
        let s = activity_in(self.locale());
        match (n, failed) {
            (_, 0) => self.toast_ok(s.masters_back, None),
            (0, _) => self.toast_err(s.masters_not_back, None),
            (n, f) => self.toast_err(format!("{}: {n}, {}: {f}", s.masters_back_n, s.masters_not_back_n), None),
        }
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Master, Track};
    use nikgob_library::NodeRow;
    use nikgob_library::replica::Held;

    use super::*;

    #[test]
    fn the_card_lists_exactly_what_goes() {
        let items = vec![
            Offered { hash: "h1".into(), path: "/m/A/01.flac".into(), label: "A — B".into(), detail: "Raspberry Pi · 30 МБ · A/01.flac".into() },
            Offered { hash: "h2".into(), path: "/m/h2.flac".into(), label: "C — D".into(), detail: "Raspberry Pi · 1,2 ГБ · h2.flac".into() },
        ];
        let c = keep_card(REPLICA_ID | 1, &items, Locale::Ru);
        assert!(c.yes_no && c.answerable());
        assert_eq!(c.options.len(), 2);
        assert_eq!(c.options[1].label, "C — D");
        assert_eq!(c.origin, "Копии есть на других устройствах · 2 файла");
        assert_eq!(c.accept_label, "Убрать в Корзину");
        assert_eq!(fmt_size_in(Locale::Ru, 30 << 20), "30 МБ");
        assert_eq!(fmt_size_in(Locale::Ru, (12 << 30) / 10), "1,2 ГБ");
        assert_eq!(fmt_size_in(Locale::En, (12 << 30) / 10), "1.2 GB");
        assert_eq!(plural_forms(Locale::Ru, 21, &activity_in(Locale::Ru).files), "файл");
        assert_eq!(plural_forms(Locale::Ru, 11, &activity_in(Locale::Ru).files), "файлов");
        assert_eq!(plural_forms(Locale::En, 2, &activity_in(Locale::En).files), "files");
    }

    /// Only a master with enough verified copies on trusted devices that
    /// this computer still has, not one kept, is offered; with the name of
    /// the device that holds it.
    #[test]
    fn only_masters_with_enough_copies_and_a_file_here_are_offered() {
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let artist = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let named = dir.path().join("Кино").join("01.flac");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        std::fs::write(&named, b"flac").unwrap();
        std::fs::write(dir.path().join("hflat.flac"), b"flac").unwrap();
        let put = |title: &str, hash: &str, path: Option<String>| {
            let t = lib.upsert_track(&Track::new(title, credit.clone())).unwrap();
            lib.put_master(&Master { track_id: t.id, codec: Codec::Flac, file_hash: hash.into(), size_bytes: 30 << 20, origin: None, file_path: path, replicas: Vec::new() })
                .unwrap();
        };
        put("Группа крови", "hnamed", Some(named.to_string_lossy().into_owned()));
        put("Кукушка", "hflat", None);
        put("Звезда", "hgone", Some(dir.path().join("gone.flac").to_string_lossy().into_owned()));
        put("Пачка сигарет", "hfew", None);
        lib.put_nodes(&[NodeRow { id: "pi".into(), name: "Raspberry Pi".into(), kind: "pi".into(), scope: "owner".into(), profile: "full".into(), ..NodeRow::default() }])
            .unwrap();
        let all: Vec<String> = ["hnamed", "hflat", "hgone"].iter().map(|s| (*s).to_owned()).collect();
        let held: Vec<Held> = all.iter().map(|h| Held { hash: h.clone(), verified_ms: 1 }).collect();
        lib.set_node_replicas("pi", &all, &held).unwrap();

        let out = offer_list(&lib, 1, &[], dir.path(), &HashSet::new(), Locale::Ru).unwrap();
        assert_eq!(out.iter().map(|o| o.hash.as_str()).collect::<Vec<_>>(), ["hflat", "hnamed"], "not one gone from here, not one without copies");
        assert_eq!(out[1].label, "Группа крови — Кино");
        assert_eq!(out[1].path, named, "the very file that would move");
        assert_eq!(out[1].detail, format!("Raspberry Pi · 30 МБ · {}", Path::new("Кино").join("01.flac").display()));
        assert_eq!(out[0].path, dir.path().join("hflat.flac"));
        assert!(offer_list(&lib, 2, &[], dir.path(), &HashSet::new(), Locale::Ru).unwrap().is_empty(), "two copies wanted");
        assert!(offer_list(&lib, 1, &["pi".into()], dir.path(), &HashSet::new(), Locale::Ru).unwrap().is_empty(), "the device is being forgotten");
        let kept: HashSet<String> = ["hflat".to_owned()].into();
        assert_eq!(offer_list(&lib, 1, &[], dir.path(), &kept, Locale::Ru).unwrap().len(), 1);
    }
}
