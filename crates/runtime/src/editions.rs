//! Editions of one album (one MusicBrainz release group) are offered to
//! merge, never merged on their own: at start-up and after each import.

use std::hash::{DefaultHasher, Hash, Hasher};

use nikgob_view::EditionOffer;

use crate::Worker;

fn offer_id(group: &str) -> u64 {
    let mut h = DefaultHasher::new();
    group.hash(&mut h);
    h.finish()
}

impl Worker {
    /// Looks for editions again and publishes the offers; a new one gets a
    /// toast.
    pub(crate) fn offer_editions(&mut self) {
        let kept = self.state.read().prefs.kept_editions.clone();
        let found = match nikgob_import::find_release_editions(&self.lib) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("looking for album editions: {e}");
                return;
            }
        };
        let found: Vec<_> = found.into_iter().filter(|g| !kept.contains(&g.group)).collect();
        let fresh = found.iter().any(|g| !self.editions.iter().any(|e| e.group == g.group));
        self.editions = found;
        let offers: Vec<EditionOffer> = self
            .editions
            .iter()
            .map(|g| EditionOffer {
                id: offer_id(&g.group),
                artist: g.keep.release.artist_credit.display(),
                keep: g.keep.release.title.clone(),
                others: g.others.iter().map(|r| r.release.title.clone()).collect(),
                same_songs: g.same_songs,
                new_songs: g.new_songs,
            })
            .collect();
        self.publish(|s, _| s.editions = offers);
        if fresh {
            self.toast("Похоже, некоторые альбомы повторяются — посмотри в Библиотеке", None);
        }
    }

    pub(crate) fn merge_editions(&mut self, id: u64) {
        let Some(g) = self.editions.iter().find(|g| offer_id(&g.group) == id).cloned() else {
            return;
        };
        match nikgob_import::fold_editions(&self.lib, &g) {
            Ok(()) => {
                self.toast(format!("Объединено в «{}»", g.keep.release.title), None);
                self.load_library();
            }
            Err(e) => {
                log::error!("merging editions: {e}");
                self.toast("Не получилось объединить альбомы", None);
            }
        }
        self.offer_editions();
    }

    pub(crate) fn keep_editions_apart(&mut self, id: u64) {
        let Some(g) = self.editions.iter().find(|g| offer_id(&g.group) == id) else {
            return;
        };
        let group = g.group.clone();
        self.publish(|s, _| s.prefs.kept_editions.push(group));
        self.save_prefs();
        self.offer_editions();
    }
}
