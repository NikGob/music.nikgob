//! «Оригиналы FLAC на ПК» (`[mesh] keep_masters`): while it is not
//! «Всегда», the lossless masters this computer still has whose verified
//! copies are on enough of the owner's trusted devices are offered on a
//! «Подтверди» card, each one listed; nothing goes by itself. On «Убрать в
//! Корзину» the devices are asked to read their copies again right then
//! (the mesh, [`Command::ReplicaRecheck`]) and only the masters that still
//! have enough go to the Trash; «Отменить» brings them back, the Trash's
//! 30 days end them. The master rows stay: the files exist on the devices.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

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

/// A master on the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Offered {
    pub(crate) hash: String,
    /// The song: «Title — Artist».
    pub(crate) label: String,
    /// Where its copies are and its size.
    pub(crate) detail: String,
}

/// What an accepted card is doing.
enum Busy {
    /// Its masters' copies are being read again on the devices.
    Rechecking { card: u64, hashes: Vec<String>, since: Instant },
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
}

/// «файл», «файла», «файлов» for `n`.
fn files_word(n: usize) -> &'static str {
    if n % 10 == 1 && n % 100 != 11 {
        "файл"
    } else if (2..5).contains(&(n % 10)) && !(12..15).contains(&(n % 100)) {
        "файла"
    } else {
        "файлов"
    }
}

/// A size in words: «820 МБ», «1,4 ГБ».
fn size_words(bytes: u64) -> String {
    const MB: f64 = (1u64 << 20) as f64;
    const GB: f64 = (1u64 << 30) as f64;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} ГБ", b / GB).replace('.', ",")
    } else {
        format!("{} МБ", (b / MB).round().max(1.0) as u64)
    }
}

/// The card of masters that may go (`id`): each one listed; yes moves
/// exactly those to the Trash, no keeps them.
pub(crate) fn keep_card(id: u64, items: &[Offered]) -> Confirm {
    let n = items.len();
    Confirm {
        id,
        title: "Оригиналы FLAC".to_owned(),
        origin: format!("Копии есть на других устройствах · {n} {}", files_word(n)),
        question: "Убрать их с компьютера в Корзину? Копии на устройствах останутся".to_owned(),
        options: items.iter().map(|o| ConfirmOption { label: o.label.clone(), detail: o.detail.clone(), recommended: false }).collect(),
        skip_label: "Оставить".to_owned(),
        accept_label: "Убрать в Корзину".to_owned(),
        pick: None,
        yes_no: true,
    }
}

/// The masters to offer: those `need` trusted devices (none of `leaving`)
/// hold verified copies of, that this computer still has (kept by name and
/// that file there, or flat in `masters_dir`), not `kept`. Reads the
/// library and the masters folder: never on the loop.
pub(crate) fn offer_list(lib: &Library, need: u32, leaving: &[String], masters_dir: &Path, kept: &HashSet<String>) -> Result<Vec<Offered>, String> {
    let all = lib.masters_to_let_go(need, leaving).map_err(|e| e.to_string())?;
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let flat = if all.iter().any(|g| g.flat) { crate::mesh::flat_masters(masters_dir) } else { HashSet::new() };
    let names: HashMap<String, String> =
        lib.nodes().map(|rows| rows.into_iter().map(|r| (r.id.clone(), r.shown_name().to_owned())).collect()).unwrap_or_default();
    let mut out = Vec::new();
    for g in all {
        if kept.contains(&g.file_hash) {
            continue;
        }
        let here = g.paths.iter().any(|p| Path::new(p).is_file()) || (g.flat && flat.contains(&g.file_hash));
        if !here {
            continue;
        }
        let label = g
            .tracks
            .first()
            .and_then(|t| lib.get_track_row(*t).ok().flatten())
            .map_or_else(|| "Без названия".to_owned(), |r| if r.artist.trim().is_empty() { r.title } else { format!("{} — {}", r.title, r.artist) });
        let on: Vec<String> = g.holders.iter().map(|h| names.get(h).cloned().unwrap_or_else(|| h.chars().take(8).collect())).collect();
        out.push(Offered { hash: g.file_hash, label, detail: format!("{} · {}", on.join(", "), size_words(g.size)) });
    }
    Ok(out)
}

impl Worker {
    /// The masters that may go are looked at again at the next tick (the
    /// choice or the devices' copies changed).
    pub(crate) fn replica_soon(&mut self) {
        self.replica.due = None;
    }

    /// The loop's tick: a look at the masters that may go when due (only
    /// while the choice lets any go); a second look the devices never
    /// answered given up.
    pub(crate) fn replica_tick(&mut self) {
        if let Some(Busy::Rechecking { since, .. }) = &self.replica.busy
            && since.elapsed() > RECHECK_WAIT
        {
            self.replica.busy = None;
            self.toast("Устройства не ответили — оригиналы остались на компьютере", None);
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
        self.replica.planning = true;
        let spawned = std::thread::Builder::new().name("nikgob-masters-plan".into()).spawn(move || {
            crate::quiet::lower_priority();
            let out = offer_list(&lib, need, &leaving, &masters_dir, &kept);
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
        let items: Vec<Offered> = items.into_iter().filter(|o| !self.replica.kept.contains(&o.hash)).take(CARD_MAX).collect();
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
        let hashes: Vec<String> = items.iter().map(|o| o.hash.clone()).collect();
        let need = self.state.read().mesh.keep_masters.copies();
        let (Some(_), Some(need)) = (option, need) else {
            self.replica.kept.extend(hashes);
            self.replica.card = None;
            self.replica.due = None;
            self.replica_publish();
            return;
        };
        let Some(link) = self.mesh.link.clone() else {
            self.toast("Межсеть не запущена — оригиналы остались на компьютере", None);
            return;
        };
        self.replica.card = None;
        self.replica.busy = Some(Busy::Rechecking { card, hashes: hashes.clone(), since: Instant::now() });
        self.replica_publish();
        let leaving = self.mesh.leaving();
        log::info!("replica: {} masters may go; their copies are read again first", hashes.len());
        link(Command::ReplicaRecheck { card, hashes, need, leaving });
    }

    /// The devices' second look: the masters that still have enough
    /// verified copies go to the Trash (exactly those of the card), the
    /// others stay.
    pub(crate) fn replica_rechecked(&mut self, card: u64, ok: Vec<String>, failed: Option<String>) {
        let asked: HashSet<String> = match &self.replica.busy {
            Some(Busy::Rechecking { card: c, hashes, .. }) if *c == card => hashes.iter().cloned().collect(),
            _ => return,
        };
        let mut ok: Vec<String> = ok.into_iter().filter(|h| asked.contains(h)).collect();
        ok.sort();
        ok.dedup();
        let stayed = asked.len() - ok.len();
        if ok.is_empty() {
            self.replica.busy = None;
            self.replica.due = None;
            if let Some(why) = failed {
                log::warn!("replica: the copies were not read again: {why}");
                self.toast("Копии не проверены — оригиналы остались на компьютере", None);
            } else {
                self.toast("Копий меньше нужного — оригиналы остались на компьютере", None);
            }
            return;
        }
        let Some(svc) = &self.import else {
            self.replica.busy = None;
            self.toast("Импорт недоступен — оригиналы остались на компьютере", None);
            return;
        };
        let (trash, masters) = (svc.config().trash_dir.clone(), svc.config().masters_dir());
        let (lib, later) = (self.lib.clone(), self.later_tx.clone());
        self.replica.busy = Some(Busy::Moving);
        let spawned = std::thread::Builder::new().name("nikgob-masters-trash".into()).spawn(move || {
            let (mut ids, mut left) = (Vec::new(), stayed);
            for h in &ok {
                match nikgob_import::trash::trash_master_files(&lib, &trash, &masters, h) {
                    Ok(moved) if moved.is_empty() => left += 1,
                    Ok(mut moved) => ids.append(&mut moved),
                    Err(e) => {
                        log::warn!("replica: the master {h} stays: {e}");
                        left += 1;
                    }
                }
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
        if ids.is_empty() {
            self.toast("Оригиналы остались на компьютере", None);
            return;
        }
        let n = ids.len();
        log::info!("replica: {n} master files moved to the Trash, {stayed} masters stay");
        let text = if stayed > 0 {
            format!("Оригиналы FLAC в Корзине: {n}, остались на компьютере: {stayed}")
        } else {
            format!("Оригиналы FLAC в Корзине: {n}")
        };
        self.toast(text, Some(("Отменить".to_owned(), Command::ReplicaUndoTrash { ids })));
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
        let text = match (n, failed) {
            (_, 0) => "Оригиналы вернулись".to_owned(),
            (0, _) => "Оригиналы не вернулись: их место занято или их нет в Корзине".to_owned(),
            (n, f) => format!("Вернулись: {n}, не вернулись: {f}"),
        };
        self.toast(text, None);
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
            Offered { hash: "h1".into(), label: "A — B".into(), detail: "Raspberry Pi · 30 МБ".into() },
            Offered { hash: "h2".into(), label: "C — D".into(), detail: "Raspberry Pi · 1,2 ГБ".into() },
        ];
        let c = keep_card(REPLICA_ID | 1, &items);
        assert!(c.yes_no && c.answerable());
        assert_eq!(c.options.len(), 2);
        assert_eq!(c.options[1].label, "C — D");
        assert_eq!(c.origin, "Копии есть на других устройствах · 2 файла");
        assert_eq!(c.accept_label, "Убрать в Корзину");
        assert_eq!(size_words(30 << 20), "30 МБ");
        assert_eq!(size_words((12 << 30) / 10), "1,2 ГБ");
        assert_eq!(files_word(21), "файл");
        assert_eq!(files_word(11), "файлов");
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

        let out = offer_list(&lib, 1, &[], dir.path(), &HashSet::new()).unwrap();
        assert_eq!(out.iter().map(|o| o.hash.as_str()).collect::<Vec<_>>(), ["hflat", "hnamed"], "not one gone from here, not one without copies");
        assert_eq!(out[1].label, "Группа крови — Кино");
        assert_eq!(out[1].detail, "Raspberry Pi · 30 МБ");
        assert!(offer_list(&lib, 2, &[], dir.path(), &HashSet::new()).unwrap().is_empty(), "two copies wanted");
        assert!(offer_list(&lib, 1, &["pi".into()], dir.path(), &HashSet::new()).unwrap().is_empty(), "the device is being forgotten");
        let kept: HashSet<String> = ["hflat".to_owned()].into();
        assert_eq!(offer_list(&lib, 1, &[], dir.path(), &kept).unwrap().len(), 1);
    }
}
